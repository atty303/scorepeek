"""Prepare and record private scorepeek-owned title-model training artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import sys
import tempfile
import uuid
from collections import Counter
from pathlib import Path
from typing import Any

from scorepeek_ocr.spike import SpikeError, _sync_directory, _write_output
from scorepeek_ocr.training_source import (
    TrainingSourceError,
    load_registered_source,
    verify_source,
)
from scorepeek_ocr.training_inputs import SPLIT_CONTRACT_ID, _split

PREPARATION_SCHEMA = "scorepeek-private-title-model-training-preparation-v1"
CROP_MAP_SCHEMA = "scorepeek-private-title-training-crop-map-v1"
EXPORT_RECORD_SCHEMA = "scorepeek-private-title-model-export-record-v1"
MAX_JSON_BYTES = 256 * 1024 * 1024
MAX_CROP_BYTES = 8 * 1024 * 1024
MAX_MODEL_FILE_BYTES = 512 * 1024 * 1024
CTC_TIMESTEP_WIDTH_FACTOR = 8


class TrainingArtifactError(Exception):
    """A title-model training or export artifact violated its contract."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _valid_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def _read(
    path: Path, expected_sha256: str, maximum: int, *, allow_empty: bool = False
) -> bytes:
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise TrainingArtifactError("input is not an absolute regular file")
    if not _valid_sha256(expected_sha256):
        raise TrainingArtifactError("input SHA-256 is invalid")
    size = path.stat().st_size
    if size < 0 or size > maximum or (size == 0 and not allow_empty):
        raise TrainingArtifactError("input size is outside the contract")
    data = path.read_bytes()
    if len(data) != size or _sha256(data) != expected_sha256:
        raise TrainingArtifactError("input changed or digest mismatched")
    return data


def _hash_unpinned_file(path: Path, name: str) -> dict[str, Any]:
    if not path.is_absolute():
        raise TrainingArtifactError(f"{name} is not an absolute regular file")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_MODEL_FILE_BYTES:
            raise TrainingArtifactError(f"{name} size or type is outside the contract")
        digest = hashlib.sha256()
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise TrainingArtifactError(f"{name} changed while reading")
            digest.update(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise TrainingArtifactError(f"{name} changed while reading")
        after = os.fstat(descriptor)
        identity = lambda value: (
            value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns
        )
        if identity(before) != identity(after):
            raise TrainingArtifactError(f"{name} changed while reading")
        return {"sha256": digest.hexdigest(), "bytes": before.st_size}
    finally:
        os.close(descriptor)


def _json(path: Path, expected_sha256: str) -> dict[str, Any]:
    try:
        value = json.loads(_read(path, expected_sha256, MAX_JSON_BYTES))
    except json.JSONDecodeError as error:
        raise TrainingArtifactError("input is invalid JSON") from error
    if not isinstance(value, dict):
        raise TrainingArtifactError("input JSON is not an object")
    return value


def _exact(value: Any, keys: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise TrainingArtifactError(f"invalid {context} fields")
    return value


def _requirements(raw: dict[str, Any]) -> tuple[str, dict[str, Any], list[str]]:
    outer = _exact(raw, {"schema", "catalog_sha256", "requirements"}, "requirements artifact")
    if outer["schema"] != "scorepeek-private-title-model-export-requirements-v1" or not _valid_sha256(
        outer["catalog_sha256"]
    ):
        raise TrainingArtifactError("requirements artifact values are invalid")
    required = _exact(
        outer["requirements"],
        {
            "schema", "baseline_dictionary_sha256", "dictionary_contract_id",
            "output_tensor_contract_id", "ctc_blank_token", "output_timesteps",
            "output_classes", "baseline_character_count",
            "appended_catalog_character_count", "non_search_variant_count",
            "covered_variant_count", "coverage_complete", "non_blank_tokens",
        },
        "model requirements",
    )
    tokens = required["non_blank_tokens"]
    if (
        required["schema"] != "scorepeek-title-model-export-requirements-v1"
        or required["dictionary_contract_id"] != "scorepeek-title-unicode-scalar-dictionary-v1"
        or required["output_tensor_contract_id"] != "scorepeek-title-ctc-f32-logits-btc-v1"
        or required["ctc_blank_token"] != 0
        or type(required["output_timesteps"]) is not int
        or required["output_timesteps"] <= 0
        or type(required["output_classes"]) is not int
        or not isinstance(tokens, list)
        or required["output_classes"] != len(tokens) + 1
        or not required["coverage_complete"]
        or required["covered_variant_count"] != required["non_search_variant_count"]
        or not _valid_sha256(required["baseline_dictionary_sha256"])
        or any(not isinstance(token, str) or len(token) != 1 or token in "\r\n" for token in tokens)
        or len(set(tokens)) != len(tokens)
        or tokens.count(" ") != 1
    ):
        raise TrainingArtifactError("model requirements are incomplete or inconsistent")
    return outer["catalog_sha256"], required, tokens


def _training_labels(raw: dict[str, Any], digest: str) -> dict[str, list[dict[str, Any]]]:
    required = {
        "schema", "split_contract_id", "candidate_artifact_sha256", "automated_label_sha256",
        "visual_audit_sha256", "final_label_sha256", "source_artifact_sha256",
        "crop_artifact_sha256", "origin", "permission_status", "provisional",
        "accepted_holdout_truth", "song_count", "label_count", "split_song_counts", "splits",
    }
    raw = _exact(raw, required, "training-input manifest")
    splits = raw["splits"]
    if (
        raw["schema"] != "scorepeek-private-title-training-input-manifest-v1"
        or raw["split_contract_id"] != SPLIT_CONTRACT_ID
        or raw["origin"] != "music_list"
        or not raw["provisional"]
        or raw["accepted_holdout_truth"]
        or raw["permission_status"] != "permission_not_recorded"
        or any(
            not _valid_sha256(raw[key])
            for key in (
                "candidate_artifact_sha256", "automated_label_sha256",
                "visual_audit_sha256", "final_label_sha256",
                "source_artifact_sha256", "crop_artifact_sha256",
            )
        )
        or type(raw["song_count"]) is not int
        or type(raw["label_count"]) is not int
        or not isinstance(raw["split_song_counts"], dict)
        or set(raw["split_song_counts"]) != {"train", "validation", "evaluation"}
        or any(type(count) is not int or count < 0 for count in raw["split_song_counts"].values())
        or not isinstance(splits, dict)
        or set(splits) != {"train", "validation", "evaluation"}
        or any(not isinstance(labels, list) for labels in splits.values())
        or sum(len(labels) for labels in splits.values()) != raw["label_count"]
        or not _valid_sha256(digest)
    ):
        raise TrainingArtifactError("training-input manifest values are invalid")
    seen_groups: set[str] = set()
    seen_song_splits: dict[str, str] = {}
    for split, labels in splits.items():
        for label in labels:
            label = _exact(
                label,
                {"group_id", "song_id", "title", "crop_pixel_sha256", "crop_file_sha256",
                 "occurrence_count", "origin", "permission_status"},
                "training label",
            )
            try:
                parsed_song_id = uuid.UUID(label["song_id"])
            except (AttributeError, ValueError) as error:
                raise TrainingArtifactError("training label song ID is invalid") from error
            if (
                not isinstance(label["group_id"], str)
                or not label["group_id"]
                or label["group_id"] in seen_groups
                or str(parsed_song_id) != label["song_id"]
                or not isinstance(label["title"], str)
                or not label["title"]
                or any(character in label["title"] for character in "\t\r\n")
                or not _valid_sha256(label["crop_pixel_sha256"])
                or not _valid_sha256(label["crop_file_sha256"])
                or type(label["occurrence_count"]) is not int
                or label["occurrence_count"] <= 0
                or label["origin"] != "music_list"
                or label["permission_status"] != "permission_not_recorded"
            ):
                raise TrainingArtifactError("training label values are invalid")
            previous = seen_song_splits.setdefault(label["song_id"], split)
            if previous != split or _split(label["song_id"]) != split:
                raise TrainingArtifactError("one song crosses training splits")
            seen_groups.add(label["group_id"])
    counts = Counter(seen_song_splits.values())
    expected_counts = {
        split: counts[split] for split in ("train", "validation", "evaluation")
    }
    if raw["song_count"] != len(seen_song_splits) or raw["split_song_counts"] != expected_counts:
        raise TrainingArtifactError("training split counts are inconsistent")
    return splits


def _prepared_manifest(raw: dict[str, Any]) -> dict[str, Any]:
    raw = _exact(
        raw,
        {
            "schema", "requirements_sha256", "training_input_sha256", "crop_map_sha256",
            "catalog_sha256", "training_source_commit", "source_training_config_sha256",
            "derived_training_config_sha256", "dictionary_contract_id", "dictionary_sha256",
            "output_tensor_contract_id", "output_timesteps", "model_input_width",
            "output_classes", "coverage_complete", "non_search_variant_count", "provisional",
            "accepted_holdout_truth", "permission_status", "split_label_counts",
            "label_file_sha256",
        },
        "training preparation",
    )
    split_names = {"train", "validation", "evaluation"}
    if (
        raw["schema"] != PREPARATION_SCHEMA
        or not raw["coverage_complete"]
        or not raw["provisional"]
        or raw["accepted_holdout_truth"]
        or raw["permission_status"] != "permission_not_recorded"
        or raw["dictionary_contract_id"] != "scorepeek-title-unicode-scalar-dictionary-v1"
        or raw["output_tensor_contract_id"] != "scorepeek-title-ctc-f32-logits-btc-v1"
        or not isinstance(raw["training_source_commit"], str)
        or len(raw["training_source_commit"]) != 40
        or any(character not in "0123456789abcdef" for character in raw["training_source_commit"])
        or type(raw["output_timesteps"]) is not int
        or raw["output_timesteps"] <= 0
        or raw["model_input_width"] != raw["output_timesteps"] * CTC_TIMESTEP_WIDTH_FACTOR
        or type(raw["output_classes"]) is not int
        or raw["output_classes"] <= 1
        or type(raw["non_search_variant_count"]) is not int
        or raw["non_search_variant_count"] <= 0
        or not isinstance(raw["split_label_counts"], dict)
        or set(raw["split_label_counts"]) != split_names
        or any(type(count) is not int or count < 0 for count in raw["split_label_counts"].values())
        or not isinstance(raw["label_file_sha256"], dict)
        or set(raw["label_file_sha256"]) != split_names
        or any(not _valid_sha256(value) for value in raw["label_file_sha256"].values())
        or any(
            not _valid_sha256(raw[key])
            for key in (
                "requirements_sha256", "training_input_sha256", "crop_map_sha256",
                "catalog_sha256", "source_training_config_sha256",
                "derived_training_config_sha256", "dictionary_sha256",
            )
        )
    ):
        raise TrainingArtifactError("training preparation values are invalid")
    return raw


def _verify_prepared_files(preparation: Path, prepared: dict[str, Any]) -> None:
    if (
        not preparation.is_absolute()
        or preparation.is_symlink()
        or not preparation.is_dir()
        or preparation.resolve() != preparation
    ):
        raise TrainingArtifactError("training preparation directory is invalid")
    _read(preparation / "dictionary.txt", prepared["dictionary_sha256"], MAX_JSON_BYTES)
    _read(
        preparation / "training-config.yml",
        prepared["derived_training_config_sha256"],
        MAX_JSON_BYTES,
    )
    for split, digest in prepared["label_file_sha256"].items():
        _read(
            preparation / f"{split}.txt", digest, MAX_JSON_BYTES, allow_empty=True
        )


def _crop_pixels(path: Path, file_sha256: str, pixel_sha256: str) -> None:
    data = _read(path, file_sha256, MAX_CROP_BYTES)
    first, separator, remainder = data.partition(b"\n")
    dimensions, separator2, remainder = remainder.partition(b"\n")
    maximum, separator3, pixels = remainder.partition(b"\n")
    if (
        first != b"P6"
        or not separator
        or not separator2
        or not separator3
        or maximum != b"255"
        or len(dimensions.split()) != 2
        or not all(part.isdigit() and int(part) > 0 for part in dimensions.split())
    ):
        raise TrainingArtifactError("crop is not a strict P6 PPM")
    width, height = (int(part) for part in dimensions.split())
    if len(pixels) != width * height * 3 or _sha256(pixels) != pixel_sha256:
        raise TrainingArtifactError("crop pixel evidence mismatched")


def _crop_map(raw: dict[str, Any], training_sha256: str) -> dict[str, dict[str, Any]]:
    raw = _exact(raw, {"schema", "training_input_sha256", "entries"}, "crop map")
    if (
        raw["schema"] != CROP_MAP_SCHEMA
        or raw["training_input_sha256"] != training_sha256
        or not isinstance(raw["entries"], list)
    ):
        raise TrainingArtifactError("crop map values are invalid")
    entries: dict[str, dict[str, Any]] = {}
    for entry in raw["entries"]:
        entry = _exact(entry, {"group_id", "path", "file_sha256", "pixel_sha256"}, "crop map entry")
        path_value = entry["path"] if isinstance(entry["path"], str) else ""
        path = Path(path_value)
        if (
            not isinstance(entry["group_id"], str)
            or not entry["group_id"]
            or entry["group_id"] in entries
            or not path.is_absolute()
            or any(character in path_value for character in "\t\r\n")
            or not _valid_sha256(entry["file_sha256"])
            or not _valid_sha256(entry["pixel_sha256"])
        ):
            raise TrainingArtifactError("crop map entry values are invalid")
        entries[entry["group_id"]] = {**entry, "path": path}
    return entries


def _derived_training_config(
    source_root: Path,
    source_config_path: str,
    output: Path,
    output_timesteps: int,
) -> bytes:
    source = (source_root / source_config_path).read_text()
    width = output_timesteps * CTC_TIMESTEP_WIDTH_FACTOR
    dictionary_path = json.dumps(str(output / "dictionary.txt"), ensure_ascii=False)
    train_path = json.dumps(str(output / "train.txt"), ensure_ascii=False)
    validation_path = json.dumps(str(output / "validation.txt"), ensure_ascii=False)
    replacements = {
        "max_text_length: &max_text_length 25": (
            f"max_text_length: &max_text_length {output_timesteps}"
        ),
        "character_dict_path: ppocr/utils/dict/ppocrv6_dict.txt": (
            f"character_dict_path: {dictionary_path}"
        ),
        "  d2s_train_image_shape: [3, 48, 320]\n": (
            f"  d2s_train_image_shape: [3, 48, {width}]\n"
        ),
        "scales: [[320, 32], [320, 48], [320, 64]]": (
            f"scales: [[{width}, 32], [{width}, 48], [{width}, 64]]"
        ),
        "image_shape: [48, 320, 3]": f"image_shape: [48, {width}, 3]",
        "        image_shape: [3, 48, 320]\n": (
            f"        image_shape: [3, 48, {width}]\n"
        ),
        "    data_dir: ./train_data/\n": "    data_dir: /\n",
        "    - ./train_data/train_list.txt\n": f"    - {train_path}\n",
        "    data_dir: ./train_data\n": "    data_dir: /\n",
        "    - ./train_data/val_list.txt\n": f"    - {validation_path}\n",
    }
    for old, new in replacements.items():
        if source.count(old) != 1:
            raise TrainingArtifactError("pinned Paddle config no longer matches the derivation contract")
        source = source.replace(old, new)
    return source.encode()


def prepare(
    requirements_path: Path,
    requirements_sha256: str,
    training_input_path: Path,
    training_input_sha256: str,
    crop_map_path: Path,
    crop_map_sha256: str,
    source_root: Path,
    output: Path,
) -> dict[str, Any]:
    catalog_sha256, requirements, tokens = _requirements(_json(requirements_path, requirements_sha256))
    splits = _training_labels(_json(training_input_path, training_input_sha256), training_input_sha256)
    crops = _crop_map(_json(crop_map_path, crop_map_sha256), training_input_sha256)
    source = load_registered_source()
    verify_source(source_root, source)
    all_labels = [label for labels in splits.values() for label in labels]
    if set(crops) != {label["group_id"] for label in all_labels}:
        raise TrainingArtifactError("crop map does not exactly cover the training labels")
    if tokens[-1] != " ":
        raise TrainingArtifactError("space must be the final Paddle dictionary token")
    dictionary = "".join(f"{token}\n" for token in tokens[:-1]).encode()
    training_config = _derived_training_config(
        source_root,
        source.small_rec_config.path,
        output,
        requirements["output_timesteps"],
    )
    token_set = set(tokens)
    label_files: dict[str, bytes] = {}
    for split, labels in splits.items():
        rows = []
        for label in labels:
            crop = crops[label["group_id"]]
            if crop["file_sha256"] != label["crop_file_sha256"] or crop["pixel_sha256"] != label["crop_pixel_sha256"]:
                raise TrainingArtifactError("crop map and training label evidence differ")
            _crop_pixels(crop["path"], crop["file_sha256"], crop["pixel_sha256"])
            if any(character not in token_set for character in label["title"]):
                raise TrainingArtifactError("training title is not covered by the model dictionary")
            rows.append(f"{crop['path']}\t{label['title']}\n")
        label_files[split] = "".join(rows).encode()
    record = {
        "schema": PREPARATION_SCHEMA,
        "requirements_sha256": requirements_sha256,
        "training_input_sha256": training_input_sha256,
        "crop_map_sha256": crop_map_sha256,
        "catalog_sha256": catalog_sha256,
        "training_source_commit": source.commit,
        "source_training_config_sha256": source.small_rec_config.sha256,
        "derived_training_config_sha256": _sha256(training_config),
        "dictionary_contract_id": requirements["dictionary_contract_id"],
        "dictionary_sha256": _sha256(dictionary),
        "output_tensor_contract_id": requirements["output_tensor_contract_id"],
        "output_timesteps": requirements["output_timesteps"],
        "model_input_width": requirements["output_timesteps"] * CTC_TIMESTEP_WIDTH_FACTOR,
        "output_classes": requirements["output_classes"],
        "coverage_complete": True,
        "non_search_variant_count": requirements["non_search_variant_count"],
        "provisional": True,
        "accepted_holdout_truth": False,
        "permission_status": "permission_not_recorded",
        "split_label_counts": {split: len(splits[split]) for split in splits},
        "label_file_sha256": {split: _sha256(label_files[split]) for split in splits},
    }
    if not output.is_absolute() or output.exists() or not output.parent.is_dir() or output.parent.is_symlink():
        raise TrainingArtifactError("output must be a new absolute directory below a regular parent")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    published = False
    try:
        os.chmod(staging, 0o700)
        (staging / "dictionary.txt").write_bytes(dictionary)
        (staging / "training-config.yml").write_bytes(training_config)
        for split, data in label_files.items():
            (staging / f"{split}.txt").write_bytes(data)
        manifest = json.dumps(record, separators=(",", ":"), allow_nan=False).encode() + b"\n"
        (staging / "manifest.json").write_bytes(manifest)
        for path in staging.iterdir():
            os.chmod(path, 0o600)
            with path.open("rb") as file:
                os.fsync(file.fileno())
        _sync_directory(staging)
        staging.rename(output)
        published = True
        _sync_directory(output.parent)
    except BaseException as error:
        cleanup_errors = []
        target = output if published else staging
        try:
            if target.exists():
                shutil.rmtree(target)
        except OSError as cleanup_error:
            cleanup_errors.append(cleanup_error)
        try:
            _sync_directory(output.parent)
        except OSError as cleanup_error:
            cleanup_errors.append(cleanup_error)
        if cleanup_errors:
            raise TrainingArtifactError(
                "training preparation publication and cleanup both failed"
            ) from error
        raise
    return {"schema": PREPARATION_SCHEMA, "output": str(output), "manifest_sha256": _sha256(manifest), **record}


def record_export(
    preparation: Path,
    preparation_sha256: str,
    paddle_model: Path,
    onnx_model: Path,
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared_manifest(_json(preparation / "manifest.json", preparation_sha256))
    _verify_prepared_files(preparation, prepared)
    files = {}
    for name, path in {"paddle_model": paddle_model, "onnx_model": onnx_model}.items():
        files[name] = _hash_unpinned_file(path, name)
    record = {
        "schema": EXPORT_RECORD_SCHEMA,
        "training_preparation_sha256": preparation_sha256,
        "catalog_sha256": prepared["catalog_sha256"],
        "training_input_sha256": prepared["training_input_sha256"],
        "dictionary_sha256": prepared["dictionary_sha256"],
        "training_source_commit": prepared["training_source_commit"],
        "source_training_config_sha256": prepared["source_training_config_sha256"],
        "derived_training_config_sha256": prepared["derived_training_config_sha256"],
        "required_output_tensor_contract_id": prepared["output_tensor_contract_id"],
        "required_output_timesteps": prepared["output_timesteps"],
        "required_output_classes": prepared["output_classes"],
        "model_shape_verified": False,
        "coverage_complete": True,
        "provisional": True,
        "distributable": False,
        "accepted_for_runtime": False,
        "files": files,
    }
    encoded = json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n"
    _write_output(output, encoded)
    return {"schema": EXPORT_RECORD_SCHEMA, "output": str(output), "artifact_sha256": _sha256(encoded.encode())}


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare")
    for name in ("requirements", "training-input", "crop-map"):
        prepare_parser.add_argument(f"--{name}", type=Path, required=True)
        prepare_parser.add_argument(f"--{name}-sha256", required=True)
    prepare_parser.add_argument("--source", type=Path, required=True)
    prepare_parser.add_argument("--output", type=Path, required=True)
    record_parser = commands.add_parser("record-export")
    record_parser.add_argument("--preparation", type=Path, required=True)
    record_parser.add_argument("--preparation-sha256", required=True)
    record_parser.add_argument("--paddle-model", type=Path, required=True)
    record_parser.add_argument("--onnx-model", type=Path, required=True)
    record_parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "prepare":
            result = prepare(
                arguments.requirements, arguments.requirements_sha256,
                arguments.training_input, arguments.training_input_sha256,
                arguments.crop_map, arguments.crop_map_sha256, arguments.source, arguments.output,
            )
        else:
            result = record_export(
                arguments.preparation, arguments.preparation_sha256,
                arguments.paddle_model, arguments.onnx_model, arguments.output,
            )
    except (OSError, SpikeError, TrainingArtifactError, TrainingSourceError) as error:
        print(f"scorepeek title-model artifact failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
