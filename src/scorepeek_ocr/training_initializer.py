"""Create and measure a dictionary-mapped PP-OCRv6 training initializer."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import os
import shutil
import stat
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import cv2
import numpy as np
import paddle
import yaml

from scorepeek_ocr.private_publication import destination_exists, publication_lock
from scorepeek_ocr.training_artifacts import (
    MAX_CROP_BYTES,
    MAX_MODEL_FILE_BYTES,
    _hash_unpinned_file,
    _prepared_manifest,
    _verify_prepared_files,
    prepared_rows,
)
from scorepeek_ocr.spike import _sync_directory
from scorepeek_ocr.training_source import load_registered_source, verify_source
from scorepeek_ocr.title_presentation import IDENTITY_TRANSFORM_ID, apply_transform

PROJECT_ROOT = Path(__file__).resolve().parents[2]
CHECKPOINT_MANIFEST = (
    PROJECT_ROOT / "models/manifests/pp-ocrv6-small-rec-pretrained-v1.json"
)
CHECKPOINT_MANIFEST_SHA256 = "edc94a7ff053901e4ea9852a5f09c5dba4014fe3b238f3eb946aeb8fe173477c"
INITIALIZER_SCHEMA = "scorepeek-private-title-model-initializer-v1"
PROBE_SCHEMA = "scorepeek-title-initializer-probe-v1"
MAX_MANIFEST_BYTES = 64 * 1024


class TrainingInitializerError(Exception):
    """The checkpoint could not be mapped to a prepared title model."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_regular(path: Path, maximum: int, expected_sha256: str | None = None) -> bytes:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= maximum:
            raise TrainingInitializerError("input size or type is outside the contract")
        chunks = []
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise TrainingInitializerError("input changed while reading")
            chunks.append(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise TrainingInitializerError("input changed while reading")
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    identity = lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns)
    data = b"".join(chunks)
    if identity(before) != identity(after) or (
        expected_sha256 is not None and _sha256(data) != expected_sha256
    ):
        raise TrainingInitializerError("input changed or digest mismatched")
    return data


def _load_checkpoint_manifest() -> dict[str, Any]:
    data = CHECKPOINT_MANIFEST.read_bytes()
    if len(data) > MAX_MANIFEST_BYTES or _sha256(data) != CHECKPOINT_MANIFEST_SHA256:
        raise TrainingInitializerError("registered checkpoint manifest digest mismatch")
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingInitializerError("registered checkpoint manifest is invalid") from error
    required = {
        "schema", "model_id", "model_name", "source_url", "sha256", "bytes",
        "license_id", "license_url", "paddleocr_version", "paddlepaddle_version",
        "training_config", "character_dictionary",
    }
    if (
        not isinstance(raw, dict)
        or set(raw) != required
        or raw["schema"] != "scorepeek-ocr-training-checkpoint-source-v1"
        or raw["model_id"] != "pp-ocrv6-small-rec-pretrained-v1"
        or raw["model_name"] != "PP-OCRv6_small_rec"
        or raw["license_id"] != "Apache-2.0"
        or raw["paddleocr_version"] != "3.7.0"
        or raw["paddlepaddle_version"] != "3.3.1"
        or not isinstance(raw["source_url"], str)
        or not raw["source_url"].startswith(
            "https://paddle-model-ecology.bj.bcebos.com/"
        )
        or not isinstance(raw["sha256"], str)
        or len(raw["sha256"]) != 64
        or not isinstance(raw["bytes"], int)
        or not 0 < raw["bytes"] <= MAX_MODEL_FILE_BYTES
        or any(
            not isinstance(raw[name], dict)
            or set(raw[name]) != {"path", "sha256"}
            or not isinstance(raw[name]["path"], str)
            or Path(raw[name]["path"]).is_absolute()
            or ".." in Path(raw[name]["path"]).parts
            or not isinstance(raw[name]["sha256"], str)
            or len(raw[name]["sha256"]) != 64
            for name in ("training_config", "character_dictionary")
        )
    ):
        raise TrainingInitializerError("registered checkpoint manifest values are invalid")
    return raw


def _tokens(
    source_root: Path, registered: dict[str, Any], target_dictionary: bytes
) -> tuple[list[str], list[str]]:
    config_record = registered["training_config"]
    dictionary_record = registered["character_dictionary"]
    config_path = source_root / config_record["path"]
    dictionary_path = source_root / dictionary_record["path"]
    config_data = _read_regular(config_path, MAX_MANIFEST_BYTES, config_record["sha256"])
    dictionary_data = _read_regular(
        dictionary_path, MAX_MANIFEST_BYTES * 2, dictionary_record["sha256"]
    )
    config = yaml.safe_load(config_data)
    if config["Global"]["character_dict_path"] != dictionary_record["path"]:
        raise TrainingInitializerError("checkpoint config does not select its registered dictionary")
    baseline = dictionary_data.decode().splitlines()
    target = target_dictionary.decode().splitlines()
    if (
        not isinstance(baseline, list)
        or any(not isinstance(token, str) or len(token) != 1 for token in baseline)
        or any(len(token) != 1 for token in target)
        or len(set(baseline)) != len(baseline)
        or len(set(target)) != len(target)
        or not set(baseline) <= set(target)
        or " " in baseline
        or " " in target
    ):
        raise TrainingInitializerError("checkpoint dictionaries cannot be mapped exactly")
    return baseline, target


def _copy_classes(destination, source, old_tokens: list[str], new_tokens: list[str], axis: int):
    old_indices = {token: index for index, token in enumerate(old_tokens)}
    new_indices = {token: index for index, token in enumerate(new_tokens)}
    if set(old_indices) - set(new_indices):
        raise TrainingInitializerError("target dictionary dropped a checkpoint token")
    result = destination.clone()
    for token, old_index in old_indices.items():
        new_index = new_indices[token]
        if axis == 0:
            result[new_index] = source[old_index]
        else:
            result[:, new_index] = source[:, old_index]
    return result


def _preprocess(
    path: str,
    width: int,
    expected_sha256: str | None = None,
    presentation_transform_id: str = IDENTITY_TRANSFORM_ID,
) -> np.ndarray:
    data = _read_regular(Path(path), MAX_CROP_BYTES, expected_sha256)
    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None:
        raise TrainingInitializerError("validation crop could not be decoded")
    image = apply_transform(image, presentation_transform_id)
    height, original_width = image.shape[:2]
    resized_width = min(width, int(math.ceil(48 * original_width / height)))
    resized = cv2.resize(image, (resized_width, 48), interpolation=cv2.INTER_LINEAR)
    normalized = (resized.astype("float32").transpose((2, 0, 1)) / 255.0 - 0.5) / 0.5
    result = np.zeros((3, 48, width), dtype=np.float32)
    result[:, :, :resized_width] = normalized
    return result


def _decode(probabilities: np.ndarray, tokens: list[str]) -> str:
    output: list[str] = []
    previous = -1
    for value in probabilities.argmax(axis=1):
        index = int(value)
        if index != 0 and index != previous:
            output.append(tokens[index])
        previous = index
    return "".join(output)


def _evaluate(
    model,
    rows: list[tuple[str, str, str]],
    tokens: list[str],
    width: int,
    presentation_transform_id: str = IDENTITY_TRANSFORM_ID,
) -> dict[str, int]:
    started = time.perf_counter()
    exact = 0
    model.eval()
    with paddle.no_grad():
        for offset in range(0, len(rows), 8):
            batch = rows[offset : offset + 8]
            images = np.stack(
                [
                    _preprocess(path, width, digest, presentation_transform_id)
                    for path, _, digest in batch
                ]
            )
            predictions = model(paddle.to_tensor(images)).numpy()
            for prediction, (_, title, _) in zip(predictions, batch, strict=True):
                exact += _decode(prediction, tokens) == title
    return {
        "sample_count": len(rows),
        "exact_count": exact,
        "elapsed_ms": round((time.perf_counter() - started) * 1000),
    }


def _publish(staging: Path, output: Path) -> None:
    published = False
    try:
        for path in staging.iterdir():
            os.chmod(path, 0o600)
            with path.open("rb") as handle:
                os.fsync(handle.fileno())
        _sync_directory(staging)
        os.chmod(staging, 0o700)
        with publication_lock(output.parent):
            if destination_exists(output):
                raise FileExistsError("private output already exists")
            staging.rename(output)
            published = True
            _sync_directory(output.parent)
    except BaseException as error:
        cleanup_errors = []
        target_path = output if published else staging
        try:
            if target_path.exists():
                shutil.rmtree(target_path)
        except OSError as cleanup_error:
            cleanup_errors.append(cleanup_error)
        try:
            _sync_directory(output.parent)
        except OSError as cleanup_error:
            cleanup_errors.append(cleanup_error)
        if cleanup_errors:
            raise TrainingInitializerError(
                "initializer publication and cleanup both failed"
            ) from error
        raise


def initialize(
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    checkpoint: Path,
    output: Path,
) -> dict[str, Any]:
    preparation_manifest = _read_regular(
        preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256
    )
    prepared = _prepared_manifest(json.loads(preparation_manifest))
    _verify_prepared_files(preparation, prepared)
    target_dictionary = _read_regular(
        preparation / "dictionary.txt", MAX_MANIFEST_BYTES * 2, prepared["dictionary_sha256"]
    )
    training_config = _read_regular(
        preparation / "training-config.yml",
        MAX_MANIFEST_BYTES,
        prepared["derived_training_config_sha256"],
    )
    validation_rows = prepared_rows(preparation, prepared, "validation")
    source = load_registered_source()
    verify_source(source_root, source)
    registered = _load_checkpoint_manifest()
    checkpoint_data = _read_regular(
        checkpoint, MAX_MODEL_FILE_BYTES, registered["sha256"]
    )
    checkpoint_file = {"sha256": _sha256(checkpoint_data), "bytes": len(checkpoint_data)}
    if checkpoint_file["bytes"] != registered["bytes"]:
        raise TrainingInitializerError("pretrained checkpoint is not registered")
    baseline, target = _tokens(source_root, registered, target_dictionary)

    sys.path.insert(0, str(source_root))
    from ppocr.modeling.architectures import build_model

    config = yaml.safe_load(training_config)
    config["Global"]["use_gpu"] = False
    ctc_tokens = ["blank", *target, " "]
    config["Architecture"]["Head"]["out_channels_list"] = {
        "CTCLabelDecode": len(ctc_tokens),
        "NRTRLabelDecode": len(ctc_tokens) + 3,
    }
    paddle.seed(0)
    model = build_model(config["Architecture"])
    current = {key: value.clone() for key, value in model.state_dict().items()}
    pretrained = paddle.load(io.BytesIO(checkpoint_data))
    if set(current) != set(pretrained):
        raise TrainingInitializerError("checkpoint tensor names do not match the prepared model")
    state = {
        key: (pretrained[key] if list(pretrained[key].shape) == list(value.shape) else value)
        for key, value in current.items()
    }
    old_ctc = ["blank", *baseline, " "]
    old_nrtr = ["blank", "<unk>", "<s>", "</s>", *baseline, " "]
    new_nrtr = ["blank", "<unk>", "<s>", "</s>", *target, " "]
    mappings = {
        "head.ctc_head.fc.bias": (old_ctc, ctc_tokens, 0),
        "head.ctc_head.fc.weight": (old_ctc, ctc_tokens, 1),
        "head.gtc_head.embedding.embedding.weight": (old_nrtr, new_nrtr, 0),
        "head.gtc_head.tgt_word_prj.weight": (old_nrtr, new_nrtr, 1),
    }
    mismatched = {
        key for key in current if list(current[key].shape) != list(pretrained[key].shape)
    }
    if mismatched != set(mappings):
        raise TrainingInitializerError("checkpoint has an unexpected tensor shape mismatch")
    for key, (old_tokens, new_tokens, axis) in mappings.items():
        state[key] = _copy_classes(current[key], pretrained[key], old_tokens, new_tokens, axis)
    model.set_state_dict(state)
    probe = _evaluate(
        model, validation_rows, ctc_tokens, prepared["model_input_width"]
    )

    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise TrainingInitializerError("output must be a new absolute directory")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        model_path = staging / "initializer.pdparams"
        paddle.save(state, str(model_path))
        model_file = _hash_unpinned_file(model_path, "initialized checkpoint")
        record = {
            "schema": INITIALIZER_SCHEMA,
            "training_preparation_sha256": preparation_sha256,
            "source_checkpoint": checkpoint_file,
            "initialized_checkpoint": model_file,
            "tensor_count": len(state),
            "shape_matched_tensor_count": len(state) - len(mappings),
            "class_mapped_tensor_count": len(mappings),
            "reused_character_count": len(baseline) + 1,
            "new_character_count": len(target) - len(baseline),
            "probe": {"schema": PROBE_SCHEMA, "split": "validation", **probe},
            "provisional": True,
            "accepted_holdout_truth": False,
            "permission_status": prepared["permission_status"],
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n"
        )
        _publish(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preparation", type=Path, required=True)
    parser.add_argument("--preparation-sha256", required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = initialize(
            arguments.preparation, arguments.preparation_sha256, arguments.source,
            arguments.checkpoint, arguments.output,
        )
    except Exception as error:
        print(f"scorepeek initializer failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
