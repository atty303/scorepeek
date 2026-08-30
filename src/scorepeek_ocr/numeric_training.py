"""Prepare and train the private fixed-ROI numeric CTC recognizer."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import shutil
import sys
import tempfile
from functools import lru_cache
from pathlib import Path
from typing import Any

import cv2
import numpy as np
import onnx
import paddle
import yaml

from scorepeek_ocr.training_artifacts import MAX_MODEL_FILE_BYTES, _hash_unpinned_file
from scorepeek_ocr.private_publication import destination_exists, publication_lock
from scorepeek_ocr.spike import _sync_directory
from scorepeek_ocr.training_initializer import MAX_MANIFEST_BYTES, _publish, _read_regular
from scorepeek_ocr.training_process import run_checked
from scorepeek_ocr.training_source import load_registered_source, verify_source

PROJECT_ROOT = Path(__file__).resolve().parents[2]
MOBILE_MANIFEST = PROJECT_ROOT / "models/manifests/en-number-mobile-v2-rec-trained-v1.json"
PPOCRV6_MANIFEST = PROJECT_ROOT / "models/manifests/pp-ocrv6-small-rec-pretrained-v1.json"
DATASET_SCHEMA = "scorepeek-private-numeric-ctc-dataset-v1"
PREPARATION_SCHEMA = "scorepeek-private-numeric-ctc-preparation-v1"
INITIALIZER_SCHEMA = "scorepeek-private-numeric-ctc-initializer-v1"
TRAINING_SCHEMA = "scorepeek-private-numeric-ctc-loso-training-v1"
EVALUATION_SCHEMA = "scorepeek-private-numeric-ctc-loso-evaluation-v1"
FINAL_TRAINING_SCHEMA = "scorepeek-private-numeric-ctc-final-training-v1"
EXPORT_SCHEMA = "scorepeek-private-numeric-ctc-onnx-export-v1"
RUNTIME_SCHEMA = "scorepeek-private-numeric-model-runtime-v1"
SENTINEL_EVALUATION_SCHEMA = "scorepeek-private-numeric-ctc-sentinel-evaluation-v2"
DICTIONARY = "0123456789-"
MAX_DATASET_BYTES = 4 * 1024 * 1024
MAX_EVALUATION_BYTES = 64 * 1024 * 1024
MAX_CROP_BYTES = 1024 * 1024
TRAINING_EPOCHS = 18
TRAINING_BATCH_SIZE = 128
TRAINING_LEARNING_RATE = 0.0005
TRAINING_EVALUATION_STEP = 20
TRAINING_TIMEOUT_SECONDS = 2 * 60 * 60
EXPORT_TIMEOUT_SECONDS = 10 * 60
ONNX_OPSET = 11
CALIBRATION_TEMPERATURES = (0.75, 1.0, 1.25, 1.5, 2.0)
FIELD_FAMILIES = {
    "level": "level",
    "notes": "notes",
    "current_score": "score",
    "previous_score": "score",
    "pgreat": "judgment",
    "great": "judgment",
    "good": "judgment",
    "bad": "judgment",
    "poor": "judgment",
    "previous_miss_count": "supplemental",
    "miss_count": "supplemental",
    "fast": "supplemental",
    "slow": "supplemental",
    "combo_break": "supplemental",
}
RUNTIME_FIELD_ORDER = (
    "level",
    "notes",
    "current_score",
    "previous_score",
    "previous_miss_count",
    "miss_count",
    "pgreat",
    "great",
    "good",
    "bad",
    "poor",
    "fast",
    "slow",
    "combo_break",
)


class NumericTrainingError(Exception):
    """The numeric CTC training contract was not satisfied."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _json(path: Path, maximum: int, expected_sha256: str | None = None) -> dict[str, Any]:
    data = _read_regular(path, maximum, expected_sha256)
    try:
        value = json.loads(data)
    except json.JSONDecodeError as error:
        raise NumericTrainingError("manifest is invalid JSON") from error
    if not isinstance(value, dict):
        raise NumericTrainingError("manifest root is not an object")
    return value


def _valid_sha256(value: object) -> bool:
    return isinstance(value, str) and len(value) == 64 and all(
        character in "0123456789abcdef" for character in value
    )


def _register_crop_binding(
    bindings: dict[str, tuple[str, str, str]], sample: dict[str, Any]
) -> None:
    digest = sample["crop_sha256"]
    binding = (sample["field"], sample["label"], sample["session_sha256"])
    existing = bindings.get(digest)
    if existing is not None:
        if existing[:2] != binding[:2]:
            raise NumericTrainingError("numeric crop digest has conflicting field or label truth")
        if existing[2] != binding[2]:
            raise NumericTrainingError("numeric crop digest crosses evaluation splits")
        raise NumericTrainingError("numeric crop digest is duplicated")
    bindings[digest] = binding


def _valid_field_label(field: object, label: object) -> bool:
    if not isinstance(field, str) or field not in FIELD_FAMILIES or not isinstance(label, str):
        return False
    maximum_digits, allows_dash, allows_leading_zeroes = _field_grammar(field)
    if label == "--":
        return allows_dash
    return (
        1 <= len(label) <= maximum_digits
        and label.isascii()
        and label.isdecimal()
        and (allows_leading_zeroes or len(label) == 1 or not label.startswith("0"))
    )


def _dataset(path: Path, expected_sha256: str) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    raw = _json(path / "manifest.json", MAX_DATASET_BYTES, expected_sha256)
    if (
        set(raw) != {"schema", "suite_sha256", "dictionary", "maximum_text_length", "samples"}
        or raw["schema"] != DATASET_SCHEMA
        or not _valid_sha256(raw["suite_sha256"])
        or raw["dictionary"] != DICTIONARY
        or raw["maximum_text_length"] != 4
        or not isinstance(raw["samples"], list)
        or not raw["samples"]
    ):
        raise NumericTrainingError("numeric dataset manifest values are invalid")
    sessions: set[str] = set()
    crop_bindings: dict[str, tuple[str, str, str]] = {}
    rows: list[dict[str, Any]] = []
    for sample in raw["samples"]:
        if (
            not isinstance(sample, dict)
            or set(sample)
            != {
                "session_sha256", "episode_id", "split", "sequence", "field",
                "label", "crop_sha256", "filename", "roi",
            }
            or not _valid_sha256(sample["session_sha256"])
            or sample["split"] != sample["session_sha256"]
            or type(sample["sequence"]) is not int
            or sample["sequence"] < 0
            or not isinstance(sample["episode_id"], str)
            or not sample["episode_id"]
            or not _valid_field_label(sample["field"], sample["label"])
            or not _valid_sha256(sample["crop_sha256"])
            or not isinstance(sample["filename"], str)
        ):
            raise NumericTrainingError("numeric dataset sample is invalid")
        relative = Path(sample["filename"])
        if relative.is_absolute() or ".." in relative.parts:
            raise NumericTrainingError("numeric dataset filename escapes its root")
        crop = _read_regular(path / relative, MAX_CROP_BYTES, sample["crop_sha256"])
        _register_crop_binding(crop_bindings, sample)
        image = cv2.imdecode(np.frombuffer(crop, dtype=np.uint8), cv2.IMREAD_COLOR)
        if image is None or image.ndim != 3 or image.shape[2] != 3:
            raise NumericTrainingError("numeric crop is not a decodable color image")
        sessions.add(sample["session_sha256"])
        rows.append({**sample, "source": str(path / relative)})
    if len(sessions) < 2:
        raise NumericTrainingError("session-disjoint evaluation needs at least two sessions")
    return raw, rows


def _augment(image: np.ndarray, seed: int) -> list[np.ndarray]:
    rng = np.random.default_rng(seed)
    height, width = image.shape[:2]
    contrast = image.astype(np.float32) * 1.08 + 6.0
    noise = rng.normal(0.0, 2.0, image.shape).astype(np.float32)
    blurred = cv2.GaussianBlur(image, (3, 3), 0.55).astype(np.float32) + noise
    down_width = max(1, round(width * 0.84))
    down = cv2.resize(image, (down_width, height), interpolation=cv2.INTER_AREA)
    down = cv2.resize(down, (width, height), interpolation=cv2.INTER_LINEAR)
    translation = np.float32([[1.0, 0.0, 1.0], [0.0, 1.0, -1.0]])
    shifted = cv2.warpAffine(
        image,
        translation,
        (width, height),
        flags=cv2.INTER_LINEAR,
        borderMode=cv2.BORDER_REPLICATE,
    )
    return [
        image,
        np.clip(contrast, 0, 255).astype(np.uint8),
        np.clip(blurred, 0, 255).astype(np.uint8),
        down,
        shifted,
    ]


def _publish_tree(staging: Path, output: Path) -> None:
    for root, directories, filenames in os.walk(staging, topdown=False):
        root_path = Path(root)
        for filename in filenames:
            path = root_path / filename
            os.chmod(path, 0o600)
            with path.open("rb") as handle:
                os.fsync(handle.fileno())
        for directory in directories:
            path = root_path / directory
            os.chmod(path, 0o700)
            _sync_directory(path)
    os.chmod(staging, 0o700)
    _sync_directory(staging)
    with publication_lock(output.parent):
        if destination_exists(output):
            raise FileExistsError("private output already exists")
        staging.rename(output)
        _sync_directory(output.parent)


def prepare(dataset: Path, dataset_sha256: str, output: Path) -> dict[str, Any]:
    raw, rows = _dataset(dataset, dataset_sha256)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    sessions = sorted({row["session_sha256"] for row in rows})
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        images = staging / "images"
        folds = staging / "folds"
        images.mkdir()
        folds.mkdir()
        prepared: list[tuple[str, str, str, str]] = []
        for index, row in enumerate(rows):
            data = _read_regular(Path(row["source"]), MAX_CROP_BYTES, row["crop_sha256"])
            image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
            assert image is not None
            for variant, augmented in enumerate(_augment(image, index)):
                ok, encoded = cv2.imencode(".png", augmented, [cv2.IMWRITE_PNG_COMPRESSION, 9])
                if not ok:
                    raise NumericTrainingError("numeric augmentation could not be encoded")
                digest = _sha256(encoded.tobytes())
                destination = images / f"{digest}.png"
                if not destination.exists():
                    destination.write_bytes(encoded.tobytes())
                prepared.append(
                    (
                        row["session_sha256"],
                        str(output / "images" / destination.name),
                        row["label"],
                        f"v{variant}",
                    )
                )
        for fold, held_out in enumerate(sessions):
            train = sorted(
                f"{path}\t{label}\n"
                for session, path, label, _ in prepared
                if session != held_out
            )
            evaluation = sorted(
                f"{path}\t{label}\n"
                for session, path, label, variant in prepared
                if session == held_out and variant == "v0"
            )
            (folds / f"fold-{fold}-train.txt").write_text("".join(train), encoding="utf-8")
            (folds / f"fold-{fold}-eval.txt").write_text("".join(evaluation), encoding="utf-8")
        (staging / "dictionary.txt").write_text("\n".join(DICTIONARY) + "\n", encoding="utf-8")
        record = {
            "schema": PREPARATION_SCHEMA,
            "dataset_sha256": dataset_sha256,
            "suite_sha256": raw["suite_sha256"],
            "dictionary": DICTIONARY,
            "image_shape": [3, 32, 320],
            "maximum_text_length": 4,
            "augmentation": {
                "generation": "scorepeek-numeric-semantic-preserving-v1",
                "variants_per_crop": 5,
                "operations": ["identity", "brightness_contrast", "blur_noise", "downscale", "roi_jitter"],
                "seed": 0,
            },
            "sessions": sessions,
            "source_samples": len(rows),
            "prepared_samples": len(prepared),
            "folds": len(sessions),
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish_tree(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def _registered_manifest(path: Path) -> dict[str, Any]:
    raw = _json(path, MAX_MANIFEST_BYTES)
    if not isinstance(raw.get("training_config"), dict) or not isinstance(
        raw.get("character_dictionary"), dict
    ):
        raise NumericTrainingError("registered initializer manifest is invalid")
    return raw


def _copy_classes(
    destination: paddle.Tensor,
    source: paddle.Tensor,
    old_tokens: list[str],
    new_tokens: list[str],
    axis: int,
) -> paddle.Tensor:
    old = {token: index for index, token in enumerate(old_tokens)}
    result = destination.clone()
    for new_index, token in enumerate(new_tokens):
        old_index = old.get(token)
        if old_index is None:
            continue
        if axis == 0:
            result[new_index] = source[old_index]
        else:
            result[:, new_index] = source[:, old_index]
    return result


def initialize(
    candidate: str,
    source_root: Path,
    checkpoint: Path,
    output: Path,
) -> dict[str, Any]:
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    manifest_path = MOBILE_MANIFEST if candidate == "mobile" else PPOCRV6_MANIFEST
    registered = _registered_manifest(manifest_path)
    checkpoint_sha = registered.get("checkpoint_sha256", registered.get("sha256"))
    checkpoint_bytes = registered.get("checkpoint_bytes", registered.get("bytes"))
    checkpoint_data = _read_regular(checkpoint, MAX_MODEL_FILE_BYTES, checkpoint_sha)
    if len(checkpoint_data) != checkpoint_bytes:
        raise NumericTrainingError("initializer checkpoint size mismatched")
    config_record = registered["training_config"]
    dictionary_record = registered["character_dictionary"]
    config_data = _read_regular(
        source_root / config_record["path"], MAX_MANIFEST_BYTES, config_record["sha256"]
    )
    dictionary_data = _read_regular(
        source_root / dictionary_record["path"], MAX_MANIFEST_BYTES * 2,
        dictionary_record["sha256"],
    )
    old_dictionary = dictionary_data.decode().splitlines()
    if not set(DICTIONARY) <= set(old_dictionary):
        raise NumericTrainingError("initializer dictionary lacks a numeric token")
    config = yaml.safe_load(config_data)
    config["Global"].update({"use_gpu": False, "use_space_char": False, "max_text_length": 4})
    architecture = config["Architecture"]
    new_ctc = ["blank", *DICTIONARY]
    old_ctc = ["blank", *old_dictionary, " "]
    if candidate == "mobile":
        architecture["Head"]["out_channels"] = len(new_ctc)
        mappings = {
            "head.fc.bias": (old_ctc, new_ctc, 0),
            "head.fc.weight": (old_ctc, new_ctc, 1),
        }
    else:
        architecture["Head"]["out_channels_list"] = {
            "CTCLabelDecode": len(new_ctc),
            "NRTRLabelDecode": len(new_ctc) + 3,
        }
        old_nrtr = ["blank", "<unk>", "<s>", "</s>", *old_dictionary, " "]
        new_nrtr = ["blank", "<unk>", "<s>", "</s>", *DICTIONARY]
        mappings = {
            "head.ctc_head.fc.bias": (old_ctc, new_ctc, 0),
            "head.ctc_head.fc.weight": (old_ctc, new_ctc, 1),
            "head.gtc_head.embedding.embedding.weight": (old_nrtr, new_nrtr, 0),
            "head.gtc_head.tgt_word_prj.weight": (old_nrtr, new_nrtr, 1),
        }
    sys.path.insert(0, str(source_root))
    from ppocr.modeling.architectures import build_model

    paddle.seed(0)
    model = build_model(architecture)
    current = {key: value.clone() for key, value in model.state_dict().items()}
    pretrained = paddle.load(io.BytesIO(checkpoint_data))
    if set(current) != set(pretrained):
        raise NumericTrainingError("initializer tensor names differ from candidate architecture")
    mismatched = {
        key for key in current if list(current[key].shape) != list(pretrained[key].shape)
    }
    if mismatched != set(mappings):
        raise NumericTrainingError(f"unexpected initializer shape mismatch: {sorted(mismatched)}")
    state = {
        key: pretrained[key] if key not in mismatched else current[key]
        for key in current
    }
    for key, (old_tokens, new_tokens, axis) in mappings.items():
        state[key] = _copy_classes(current[key], pretrained[key], old_tokens, new_tokens, axis)
    model.set_state_dict(state)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        checkpoint_path = staging / "initializer.pdparams"
        paddle.save(state, str(checkpoint_path))
        checkpoint_record = _hash_unpinned_file(checkpoint_path, "numeric initializer")
        record = {
            "schema": INITIALIZER_SCHEMA,
            "candidate": candidate,
            "source_model_id": registered["model_id"],
            "source_checkpoint_sha256": checkpoint_sha,
            "training_source_commit": source.commit,
            "dictionary": DICTIONARY,
            "image_shape": [3, 32, 320],
            "checkpoint": checkpoint_record,
            "tensor_count": len(state),
            "class_mapped_tensor_count": len(mappings),
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def _prepared(path: Path, expected_sha256: str) -> dict[str, Any]:
    raw = _json(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    if (
        set(raw)
        != {
            "schema", "dataset_sha256", "suite_sha256", "dictionary", "image_shape",
            "maximum_text_length", "augmentation", "sessions", "source_samples",
            "prepared_samples", "folds",
        }
        or raw["schema"] != PREPARATION_SCHEMA
        or raw["dictionary"] != DICTIONARY
        or raw["image_shape"] != [3, 32, 320]
        or raw["maximum_text_length"] != 4
        or not isinstance(raw["sessions"], list)
        or len(raw["sessions"]) != raw["folds"]
        or len(raw["sessions"]) < 2
    ):
        raise NumericTrainingError("numeric preparation manifest is invalid")
    for fold in range(raw["folds"]):
        for split in ("train", "eval"):
            data = _read_regular(
                path / "folds" / f"fold-{fold}-{split}.txt", MAX_DATASET_BYTES
            )
            for line in data.decode().splitlines():
                filename, separator, label = line.partition("\t")
                if (
                    separator != "\t"
                    or not Path(filename).is_file()
                    or not label
                    or any(token not in DICTIONARY for token in label)
                ):
                    raise NumericTrainingError("numeric preparation list is invalid")
    return raw


def _initializer(path: Path, expected_sha256: str, candidate: str) -> dict[str, Any]:
    raw = _json(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    if (
        set(raw)
        != {
            "schema", "candidate", "source_model_id", "source_checkpoint_sha256",
            "training_source_commit", "dictionary", "image_shape", "checkpoint",
            "tensor_count", "class_mapped_tensor_count",
        }
        or raw["schema"] != INITIALIZER_SCHEMA
        or raw["candidate"] != candidate
        or raw["dictionary"] != DICTIONARY
        or raw["image_shape"] != [3, 32, 320]
        or not isinstance(raw["checkpoint"], dict)
        or set(raw["checkpoint"]) != {"sha256", "bytes"}
    ):
        raise NumericTrainingError("numeric initializer manifest is invalid")
    data = _read_regular(
        path / "initializer.pdparams", MAX_MODEL_FILE_BYTES, raw["checkpoint"]["sha256"]
    )
    if len(data) != raw["checkpoint"]["bytes"]:
        raise NumericTrainingError("numeric initializer checkpoint size mismatched")
    return raw


def _training_config(
    candidate: str,
    source_root: Path,
    preparation: Path,
    initializer: Path,
    fold: int,
    output: Path,
) -> dict[str, Any]:
    registered = _registered_manifest(
        MOBILE_MANIFEST if candidate == "mobile" else PPOCRV6_MANIFEST
    )
    config_record = registered["training_config"]
    config = yaml.safe_load(
        _read_regular(
            source_root / config_record["path"],
            MAX_MANIFEST_BYTES,
            config_record["sha256"],
        )
    )
    config["Global"].update(
        {
            "use_gpu": False,
            "distributed": False,
            "epoch_num": TRAINING_EPOCHS,
            "print_batch_step": 20,
            "save_model_dir": str(output),
            "save_epoch_step": 1,
            "eval_batch_step": [0, TRAINING_EVALUATION_STEP],
            "cal_metric_during_train": False,
            "pretrained_model": str(initializer / "initializer.pdparams"),
            "checkpoints": None,
            "use_visualdl": False,
            "seed": 0,
            "character_dict_path": str(preparation / "dictionary.txt"),
            "use_space_char": False,
            "max_text_length": 4,
        }
    )
    config["Optimizer"]["lr"] = {
        "name": "Const",
        "learning_rate": TRAINING_LEARNING_RATE,
    }
    architecture = config["Architecture"]
    if candidate == "mobile":
        architecture["Head"]["out_channels"] = len(DICTIONARY) + 1
        train_transforms = [
            {"DecodeImage": {"img_mode": "BGR", "channel_first": False}},
            {"CTCLabelEncode": None},
            {"RecResizeImg": {"image_shape": [3, 32, 320]}},
            {"KeepKeys": {"keep_keys": ["image", "label", "length"]}},
        ]
        eval_transforms = train_transforms
    else:
        architecture["Head"]["out_channels_list"] = {
            "CTCLabelDecode": len(DICTIONARY) + 1,
            "NRTRLabelDecode": len(DICTIONARY) + 4,
        }
        train_transforms = [
            {"DecodeImage": {"img_mode": "BGR", "channel_first": False}},
            {"MultiLabelEncode": {"gtc_encode": "NRTRLabelEncode"}},
            {"RecResizeImg": {"image_shape": [3, 32, 320]}},
            {
                "KeepKeys": {
                    "keep_keys": [
                        "image", "label_ctc", "label_gtc", "length", "valid_ratio"
                    ]
                }
            },
        ]
        eval_transforms = train_transforms
    config["Train"] = {
        "dataset": {
            "name": "SimpleDataSet",
            "data_dir": "/",
            "label_file_list": [str(preparation / "folds" / f"fold-{fold}-train.txt")],
            "ratio_list": [1.0],
            "transforms": train_transforms,
        },
        "loader": {
            "shuffle": True,
            "batch_size_per_card": TRAINING_BATCH_SIZE,
            "drop_last": False,
            "num_workers": 0,
        },
    }
    config["Eval"] = {
        "dataset": {
            "name": "SimpleDataSet",
            "data_dir": "/",
            "label_file_list": [str(preparation / "folds" / f"fold-{fold}-eval.txt")],
            "transforms": eval_transforms,
        },
        "loader": {
            "shuffle": False,
            "batch_size_per_card": TRAINING_BATCH_SIZE,
            "drop_last": False,
            "num_workers": 0,
        },
    }
    return config


def train_loso(
    candidate: str,
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    initializer: Path,
    initializer_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared(preparation, preparation_sha256)
    initialized = _initializer(initializer, initializer_sha256, candidate)
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        folds = []
        environment = os.environ.copy()
        environment["NO_ALBUMENTATIONS_UPDATE"] = "1"
        environment["OMP_NUM_THREADS"] = "8"
        for fold, held_out in enumerate(prepared["sessions"]):
            fold_root = staging / f"fold-{fold}"
            fold_root.mkdir()
            model_root = fold_root / "model"
            config = _training_config(
                candidate, source_root, preparation, initializer, fold, model_root
            )
            config_path = fold_root / "training-config.yml"
            config_path.write_text(
                yaml.safe_dump(config, allow_unicode=True, sort_keys=False),
                encoding="utf-8",
            )
            run_checked(
                [
                    sys.executable,
                    str(source_root / source.training_entrypoint.path),
                    "-c",
                    str(config_path),
                ],
                cwd=source_root,
                environment=environment,
                timeout_seconds=TRAINING_TIMEOUT_SECONDS,
            )
            selected = model_root / "best_accuracy.pdparams"
            if not selected.is_file():
                selected = model_root / "latest.pdparams"
            checkpoint = _hash_unpinned_file(selected, "numeric LOSO checkpoint")
            destination = fold_root / "selected.pdparams"
            shutil.copyfile(selected, destination)
            folds.append({"fold": fold, "held_out_session": held_out, "checkpoint": checkpoint})
            shutil.rmtree(model_root)
        record = {
            "schema": TRAINING_SCHEMA,
            "candidate": candidate,
            "preparation_sha256": preparation_sha256,
            "initializer_manifest_sha256": initializer_sha256,
            "initializer_checkpoint_sha256": initialized["checkpoint"]["sha256"],
            "training_source_commit": source.commit,
            "recipe": {
                "epochs": TRAINING_EPOCHS,
                "batch_size": TRAINING_BATCH_SIZE,
                "learning_rate": TRAINING_LEARNING_RATE,
                "learning_rate_schedule": "constant",
                "device": "cpu",
                "seed": 0,
                "runtime_augmentation": False,
                "evaluation_step": TRAINING_EVALUATION_STEP,
            },
            "folds": folds,
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish_tree(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def _training(path: Path, expected_sha256: str, candidate: str) -> dict[str, Any]:
    raw = _json(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    if (
        set(raw)
        != {
            "schema", "candidate", "preparation_sha256", "initializer_manifest_sha256",
            "initializer_checkpoint_sha256", "training_source_commit", "recipe", "folds",
        }
        or raw["schema"] != TRAINING_SCHEMA
        or raw["candidate"] != candidate
        or not isinstance(raw["folds"], list)
        or not raw["folds"]
    ):
        raise NumericTrainingError("numeric LOSO training manifest is invalid")
    for fold in raw["folds"]:
        if (
            not isinstance(fold, dict)
            or set(fold) != {"fold", "held_out_session", "checkpoint"}
            or not isinstance(fold["checkpoint"], dict)
            or set(fold["checkpoint"]) != {"sha256", "bytes"}
        ):
            raise NumericTrainingError("numeric LOSO fold is invalid")
        checkpoint = _read_regular(
            path / f"fold-{fold['fold']}" / "selected.pdparams",
            MAX_MODEL_FILE_BYTES,
            fold["checkpoint"]["sha256"],
        )
        if len(checkpoint) != fold["checkpoint"]["bytes"]:
            raise NumericTrainingError("numeric LOSO checkpoint size mismatched")
    return raw


def _preprocess_numeric(path: Path, expected_sha256: str) -> np.ndarray:
    data = _read_regular(path, MAX_CROP_BYTES, expected_sha256)
    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None:
        raise NumericTrainingError("numeric evaluation crop could not be decoded")
    height, width = image.shape[:2]
    resized_width = min(320, int(np.ceil(32 * width / height)))
    resized = cv2.resize(image, (resized_width, 32), interpolation=cv2.INTER_LINEAR)
    normalized = (resized.astype(np.float32).transpose((2, 0, 1)) / 255.0 - 0.5) / 0.5
    result = np.zeros((3, 32, 320), dtype=np.float32)
    result[:, :, :resized_width] = normalized
    return result


def _greedy_decode(probabilities: np.ndarray) -> str:
    output: list[str] = []
    previous = -1
    for raw in probabilities.argmax(axis=1):
        index = int(raw)
        if index != 0 and index != previous:
            output.append(DICTIONARY[index - 1])
        previous = index
    return "".join(output)


@lru_cache(maxsize=8)
def _numeric_trie(
    maximum_digits: int, allows_dash: bool, allows_leading_zeroes: bool
) -> tuple[np.ndarray, np.ndarray, list[str]]:
    parents = [0]
    tokens = [0]
    labels = [""]
    children: dict[tuple[int, int], int] = {}

    def insert(text: str, encoded: list[int]) -> None:
        node = 0
        for token in encoded:
            key = (node, token)
            child = children.get(key)
            if child is None:
                child = len(parents)
                children[key] = child
                parents.append(node)
                tokens.append(token)
                labels.append("")
            node = child
        labels[node] = text

    for length in range(1, maximum_digits + 1):
        first = 0 if length == 1 or allows_leading_zeroes else 10 ** (length - 1)
        for value in range(first, 10**length):
            text = f"{value:0{length}d}"
            insert(text, [int(character) + 1 for character in text])
    if allows_dash:
        insert("--", [11, 11])
    return np.asarray(parents), np.asarray(tokens), labels


def _field_grammar(field: str) -> tuple[int, bool, bool]:
    return (
        2 if field == "level" else 3 if field == "combo_break" else 4,
        field in {
            "previous_score", "previous_miss_count", "miss_count", "fast", "slow",
            "combo_break",
        },
        field == "notes",
    )


def _logsumexp_pair(left: np.ndarray, right: np.ndarray) -> np.ndarray:
    maximum = np.maximum(left, right)
    with np.errstate(invalid="ignore"):
        result = maximum + np.log(np.exp(left - maximum) + np.exp(right - maximum))
    return np.where(np.isneginf(maximum), maximum, result)


def _exact_ctc_candidates(
    probabilities: np.ndarray, field: str, temperature: float
) -> dict[str, Any]:
    maximum_digits, allows_dash, allows_leading_zeroes = _field_grammar(field)
    parents, tokens, labels = _numeric_trie(
        maximum_digits, allows_dash, allows_leading_zeroes
    )
    scaled = np.log(np.maximum(probabilities.astype(np.float64), np.finfo(np.float64).tiny))
    scaled /= temperature
    scaled -= np.logaddexp.reduce(scaled, axis=1, keepdims=True)
    blank = np.full(len(parents), -np.inf)
    nonblank = np.full(len(parents), -np.inf)
    blank[0] = 0.0
    for row in scaled:
        next_blank = _logsumexp_pair(blank, nonblank) + row[0]
        next_nonblank = np.full(len(parents), -np.inf)
        parent_blank = blank[parents[1:]]
        parent_nonblank = nonblank[parents[1:]]
        repeated = tokens[1:] == tokens[parents[1:]]
        sources = _logsumexp_pair(nonblank[1:], parent_blank)
        sources = _logsumexp_pair(sources, np.where(repeated, -np.inf, parent_nonblank))
        next_nonblank[1:] = sources + row[tokens[1:]]
        blank, nonblank = next_blank, next_nonblank
    scores = _logsumexp_pair(blank, nonblank)
    value_nodes = np.asarray([index for index, label in enumerate(labels) if label])
    order = sorted(
        (int(node) for node in value_nodes),
        key=lambda node: (-scores[node], labels[node]),
    )
    normalizer = np.logaddexp.reduce(np.concatenate(([scores[0]], scores[value_nodes])))
    candidates = [
        {
            "text": labels[int(node)],
            "log_probability": float(scores[node]),
            "calibrated_probability": float(np.exp(scores[node] - normalizer)),
        }
        for node in order[:8]
    ]
    return {
        "candidates": candidates,
        "all_blank_log_probability": float(scores[0]),
        "runner_up_margin": (
            None if len(candidates) < 2
            else candidates[0]["log_probability"] - candidates[1]["log_probability"]
        ),
    }


def _select_zero_error_calibration(
    rows: list[dict[str, Any]], family: str
) -> dict[str, Any]:
    selected: dict[str, Any] | None = None
    for temperature in CALIBRATION_TEMPERATURES:
        observations = [
            row["temperatures"][str(temperature)]
            for row in rows
            if FIELD_FAMILIES[row["field"]] == family
        ]
        probability_values = sorted(
            {0.0, *(item["candidates"][0]["calibrated_probability"] for item in observations)}
        )
        margin_values = sorted(
            {0.0, *(item["runner_up_margin"] or 0.0 for item in observations)}
        )
        for minimum_probability in probability_values:
            for minimum_margin in margin_values:
                accepted = [
                    item for item in observations
                    if item["candidates"]
                    and item["candidates"][0]["log_probability"]
                        > item["all_blank_log_probability"]
                    and item["candidates"][0]["calibrated_probability"]
                        >= minimum_probability
                    and (item["runner_up_margin"] or 0.0) >= minimum_margin
                ]
                incorrect = sum(not item["correct"] for item in accepted)
                correct = sum(item["correct"] for item in accepted)
                candidate = {
                    "temperature": temperature,
                    "minimum_probability": minimum_probability,
                    "minimum_runner_up_margin": minimum_margin,
                    "accepted_correct": correct,
                    "accepted_incorrect": incorrect,
                    "observations": len(observations),
                }
                if incorrect == 0 and (
                    selected is None
                    or (correct, -minimum_probability, -minimum_margin, -temperature)
                    > (
                        selected["accepted_correct"],
                        -selected["minimum_probability"],
                        -selected["minimum_runner_up_margin"],
                        -selected["temperature"],
                    )
                ):
                    selected = candidate
    if selected is None:
        raise NumericTrainingError(f"no zero-error calibration exists for {family}")
    return selected


def _joint_score_decision(fields: dict[str, dict[str, Any]]) -> dict[str, Any] | None:
    required = {"current_score", "pgreat", "great"}
    if not required <= set(fields):
        return None
    if not all(
        fields[field]["exact_ctc"]["candidates"]
        and fields[field]["exact_ctc"]["candidates"][0]["log_probability"]
            > fields[field]["exact_ctc"]["all_blank_log_probability"]
        for field in required
    ):
        return None
    if not all(
        fields[field].get("calibration_accepted", fields[field]["accepted"])
        for field in ("pgreat", "great")
    ):
        return None
    notes = None
    if "notes" in fields and fields["notes"].get(
        "calibration_accepted", fields["notes"]["accepted"]
    ):
        try:
            notes = int(fields["notes"]["exact_ctc"]["candidates"][0]["text"])
        except (IndexError, ValueError):
            return None
    candidates = []
    for score in fields["current_score"]["exact_ctc"]["candidates"]:
        for pgreat in fields["pgreat"]["exact_ctc"]["candidates"]:
            for great in fields["great"]["exact_ctc"]["candidates"]:
                try:
                    values = tuple(int(item["text"]) for item in (score, pgreat, great))
                except ValueError:
                    continue
                score_value, pgreat_value, great_value = values
                if (
                    (notes is None or score_value <= 2 * notes)
                    and (notes is None or pgreat_value <= notes)
                    and (notes is None or great_value <= notes)
                    and score_value == 2 * pgreat_value + great_value
                ):
                    candidates.append(
                        {
                            "current_score": score_value,
                            "pgreat": pgreat_value,
                            "great": great_value,
                            "joint_log_probability": sum(
                                item["log_probability"] for item in (score, pgreat, great)
                            ),
                        }
                    )
    candidates.sort(
        key=lambda item: (
            -item["joint_log_probability"],
            item["current_score"], item["pgreat"], item["great"],
        )
    )
    if not candidates:
        return None
    truth = (
        int(fields["current_score"]["truth"]),
        int(fields["pgreat"]["truth"]),
        int(fields["great"]["truth"]),
    )
    selected = candidates[0]
    return {
        "candidates": candidates[:8],
        "runner_up_margin": (
            None if len(candidates) < 2
            else selected["joint_log_probability"] - candidates[1]["joint_log_probability"]
        ),
        "correct": (
            selected["current_score"], selected["pgreat"], selected["great"]
        ) == truth,
    }


def _select_zero_error_temporal_calibration(
    predictions: list[dict[str, Any]], family: str, temperature: float
) -> dict[str, Any] | None:
    grouped: dict[tuple[str, str, str], list[dict[str, Any]]] = {}
    for prediction in predictions:
        if FIELD_FAMILIES[prediction["field"]] == family:
            key = (
                prediction["session_sha256"],
                prediction["episode_id"],
                prediction["field"],
            )
            grouped.setdefault(key, []).append(prediction)
    pairs = []
    for observations in grouped.values():
        observations.sort(key=lambda item: item["sequence"])
        for first, second in zip(observations, observations[1:]):
            left = first["exact_ctc"]
            right = second["exact_ctc"]
            if (
                not left["candidates"]
                or not right["candidates"]
                or left["candidates"][0]["text"] != right["candidates"][0]["text"]
                or left["candidates"][0]["log_probability"]
                    <= left["all_blank_log_probability"]
                or right["candidates"][0]["log_probability"]
                    <= right["all_blank_log_probability"]
            ):
                continue
            pairs.append(
                {
                    "minimum_probability": min(
                        left["candidates"][0]["calibrated_probability"],
                        right["candidates"][0]["calibrated_probability"],
                    ),
                    "minimum_runner_up_margin": min(
                        left["runner_up_margin"] or 0.0,
                        right["runner_up_margin"] or 0.0,
                    ),
                    "correct": bool(
                        first["correct"]
                        and second["correct"]
                        and first["truth"] == second["truth"]
                    ),
                }
            )
    selected = None
    for minimum_probability in sorted(
        {0.0, *(pair["minimum_probability"] for pair in pairs)}
    ):
        for minimum_margin in sorted(
            {0.0, *(pair["minimum_runner_up_margin"] for pair in pairs)}
        ):
            accepted = [
                pair
                for pair in pairs
                if pair["minimum_probability"] >= minimum_probability
                and pair["minimum_runner_up_margin"] >= minimum_margin
            ]
            correct = sum(pair["correct"] for pair in accepted)
            incorrect = sum(not pair["correct"] for pair in accepted)
            candidate = {
                "temperature": temperature,
                "minimum_probability": minimum_probability,
                "minimum_runner_up_margin": minimum_margin,
                "accepted_correct": correct,
                "accepted_incorrect": incorrect,
                "observations": len(pairs),
            }
            if correct > 0 and incorrect == 0 and (
                selected is None
                or (correct, -minimum_probability, -minimum_margin)
                    > (
                        selected["accepted_correct"],
                        -selected["minimum_probability"],
                        -selected["minimum_runner_up_margin"],
                    )
            ):
                selected = candidate
    return selected


def _select_zero_error_temporal_tuple_calibration(
    predictions: list[dict[str, Any]], fields: set[str], temperature: float
) -> dict[str, Any] | None:
    observations: dict[tuple[str, str, int], dict[str, dict[str, Any]]] = {}
    for prediction in predictions:
        if prediction["field"] in fields:
            key = (
                prediction["session_sha256"],
                prediction["episode_id"],
                prediction["sequence"],
            )
            observations.setdefault(key, {})[prediction["field"]] = prediction
    grouped: dict[tuple[str, str], list[tuple[int, dict[str, dict[str, Any]]]]] = {}
    for (session, episode, sequence), values in observations.items():
        if set(values) == fields:
            grouped.setdefault((session, episode), []).append((sequence, values))
    pairs = []
    for episode in grouped.values():
        episode.sort(key=lambda item: item[0])
        for (_, first), (_, second) in zip(episode, episode[1:]):
            inferences = [
                values[field]["exact_ctc"]
                for values in (first, second)
                for field in sorted(fields)
            ]
            if any(
                not inference["candidates"]
                or inference["candidates"][0]["log_probability"]
                    <= inference["all_blank_log_probability"]
                for inference in inferences
            ) or any(
                first[field]["exact_ctc"]["candidates"][0]["text"]
                    != second[field]["exact_ctc"]["candidates"][0]["text"]
                for field in fields
            ):
                continue
            pairs.append(
                {
                    "minimum_probability": min(
                        inference["candidates"][0]["calibrated_probability"]
                        for inference in inferences
                    ),
                    "minimum_runner_up_margin": min(
                        inference["runner_up_margin"] or 0.0 for inference in inferences
                    ),
                    "correct": all(
                        values[field]["correct"]
                        for values in (first, second)
                        for field in fields
                    ),
                }
            )
    selected = None
    for minimum_probability in sorted(
        {0.0, *(pair["minimum_probability"] for pair in pairs)}
    ):
        for minimum_margin in sorted(
            {0.0, *(pair["minimum_runner_up_margin"] for pair in pairs)}
        ):
            accepted = [
                pair for pair in pairs
                if pair["minimum_probability"] >= minimum_probability
                and pair["minimum_runner_up_margin"] >= minimum_margin
            ]
            correct = sum(pair["correct"] for pair in accepted)
            incorrect = sum(not pair["correct"] for pair in accepted)
            candidate = {
                "temperature": temperature,
                "minimum_probability": minimum_probability,
                "minimum_runner_up_margin": minimum_margin,
                "accepted_correct": correct,
                "accepted_incorrect": incorrect,
                "observations": len(pairs),
            }
            if correct > 0 and incorrect == 0 and (
                selected is None
                or (correct, -minimum_probability, -minimum_margin)
                    > (
                        selected["accepted_correct"],
                        -selected["minimum_probability"],
                        -selected["minimum_runner_up_margin"],
                    )
            ):
                selected = candidate
    return selected


def _select_zero_error_temporal_joint_calibration(
    joint_rows: list[tuple[tuple[str, str, int], dict[str, Any]]],
) -> dict[str, Any] | None:
    grouped: dict[tuple[str, str], list[tuple[int, dict[str, Any]]]] = {}
    for (session_sha256, episode_id, sequence), decision in joint_rows:
        grouped.setdefault((session_sha256, episode_id), []).append((sequence, decision))
    pairs = []
    for observations in grouped.values():
        observations.sort(key=lambda item: item[0])
        for (_, first), (_, second) in zip(observations, observations[1:]):
            first_selected = first["candidates"][0]
            second_selected = second["candidates"][0]
            selected_fields = ("current_score", "pgreat", "great")
            if any(first_selected[field] != second_selected[field] for field in selected_fields):
                continue
            margins = [
                margin
                for margin in (first["runner_up_margin"], second["runner_up_margin"])
                if margin is not None
            ]
            pairs.append(
                {
                    "minimum_runner_up_margin": min(margins) if margins else None,
                    "correct": bool(first["correct"] and second["correct"]),
                }
            )
    selected = None
    thresholds = {0.0}
    thresholds.update(
        pair["minimum_runner_up_margin"]
        for pair in pairs
        if pair["minimum_runner_up_margin"] is not None
    )
    for threshold in sorted(thresholds):
        accepted = [
            pair
            for pair in pairs
            if pair["minimum_runner_up_margin"] is None
            or pair["minimum_runner_up_margin"] >= threshold
        ]
        correct = sum(pair["correct"] for pair in accepted)
        incorrect = sum(not pair["correct"] for pair in accepted)
        candidate = {
            "minimum_runner_up_margin": threshold,
            "accepted_correct": correct,
            "accepted_incorrect": incorrect,
            "observations": len(pairs),
        }
        if correct > 0 and incorrect == 0 and (
            selected is None
            or (correct, -threshold)
                > (selected["accepted_correct"], -selected["minimum_runner_up_margin"])
        ):
            selected = candidate
    return selected


def evaluate_loso(
    candidate: str,
    dataset: Path,
    dataset_sha256: str,
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    training: Path,
    training_sha256: str,
    output: Path,
) -> dict[str, Any]:
    _, rows = _dataset(dataset, dataset_sha256)
    prepared = _prepared(preparation, preparation_sha256)
    trained = _training(training, training_sha256, candidate)
    source = load_registered_source()
    verify_source(source_root, source)
    if (
        trained["preparation_sha256"] != preparation_sha256
        or trained["training_source_commit"] != source.commit
    ):
        raise NumericTrainingError("numeric LOSO training lineage differs")
    if [fold["held_out_session"] for fold in trained["folds"]] != prepared["sessions"]:
        raise NumericTrainingError("numeric LOSO held-out sessions differ")
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    sys.path.insert(0, str(source_root))
    from ppocr.modeling.architectures import build_model

    predictions: list[dict[str, Any]] = []
    for fold in trained["folds"]:
        selected_rows = [
            row for row in rows if row["session_sha256"] == fold["held_out_session"]
        ]
        config = _training_config(
            candidate,
            source_root,
            preparation,
            training,
            int(fold["fold"]),
            output,
        )
        model = build_model(config["Architecture"])
        model.set_state_dict(
            paddle.load(str(training / f"fold-{fold['fold']}" / "selected.pdparams"))
        )
        model.eval()
        with paddle.no_grad():
            for offset in range(0, len(selected_rows), TRAINING_BATCH_SIZE):
                batch = selected_rows[offset : offset + TRAINING_BATCH_SIZE]
                inputs = np.stack(
                    [
                        _preprocess_numeric(Path(row["source"]), row["crop_sha256"])
                        for row in batch
                    ]
                )
                probabilities = model(paddle.to_tensor(inputs)).numpy()
                for row, probability in zip(batch, probabilities, strict=True):
                    predicted = _greedy_decode(probability)
                    temperatures = {}
                    for temperature in CALIBRATION_TEMPERATURES:
                        inference = _exact_ctc_candidates(
                            probability, row["field"], temperature
                        )
                        inference["correct"] = bool(
                            inference["candidates"]
                            and inference["candidates"][0]["text"] == row["label"]
                        )
                        temperatures[str(temperature)] = inference
                    predictions.append(
                        {
                            "session_sha256": row["session_sha256"],
                            "episode_id": row["episode_id"],
                            "sequence": row["sequence"],
                            "field": row["field"],
                            "truth": row["label"],
                            "predicted": predicted,
                            "correct": predicted == row["label"],
                            "temperatures": temperatures,
                        }
                    )
    calibration = {}
    for family in sorted(set(FIELD_FAMILIES.values())):
        if any(FIELD_FAMILIES[prediction["field"]] == family for prediction in predictions):
            calibration[family] = {
                "enabled": True,
                **_select_zero_error_calibration(predictions, family),
            }
        else:
            calibration[family] = {
                "enabled": False,
                "temperature": 1.0,
                "minimum_probability": 1.0,
                "minimum_runner_up_margin": 0.0,
                "accepted_correct": 0,
                "accepted_incorrect": 0,
                "observations": 0,
            }
    for prediction in predictions:
        selected = calibration[FIELD_FAMILIES[prediction["field"]]]
        inference = prediction.pop("temperatures")[str(selected["temperature"])]
        prediction["exact_ctc"] = {
            key: value for key, value in inference.items() if key != "correct"
        }
        prediction["accepted"] = bool(
            inference["candidates"]
            and inference["candidates"][0]["log_probability"]
                > inference["all_blank_log_probability"]
            and inference["candidates"][0]["calibrated_probability"]
                >= selected["minimum_probability"]
            and (inference["runner_up_margin"] or 0.0)
                >= selected["minimum_runner_up_margin"]
        )
        prediction["accepted_text"] = (
            inference["candidates"][0]["text"] if prediction["accepted"] else None
        )
        prediction["calibration_accepted"] = prediction["accepted"]
    temporal_calibration = {}
    for family in sorted(set(FIELD_FAMILIES.values())):
        selected = None
        if calibration[family]["enabled"]:
            selected = (
                _select_zero_error_temporal_tuple_calibration(
                    predictions,
                    {"pgreat", "great", "good", "bad", "poor"},
                    calibration[family]["temperature"],
                )
                if family == "judgment"
                else _select_zero_error_temporal_calibration(
                    predictions, family, calibration[family]["temperature"]
                )
            )
        temporal_calibration[family] = (
            {"enabled": True, **selected} if selected is not None else calibration[family]
        )
    for prediction in predictions:
        selected = temporal_calibration[FIELD_FAMILIES[prediction["field"]]]
        inference = prediction["exact_ctc"]
        prediction["accepted"] = bool(
            selected["enabled"]
            and inference["candidates"]
            and inference["candidates"][0]["log_probability"]
                > inference["all_blank_log_probability"]
            and inference["candidates"][0]["calibrated_probability"]
                >= selected["minimum_probability"]
            and (inference["runner_up_margin"] or 0.0)
                >= selected["minimum_runner_up_margin"]
        )
        prediction["accepted_text"] = (
            inference["candidates"][0]["text"] if prediction["accepted"] else None
        )
        prediction["calibration_accepted"] = prediction["accepted"]
    per_field: dict[str, dict[str, int]] = {}
    for prediction in predictions:
        field = per_field.setdefault(prediction["field"], {"samples": 0, "correct": 0})
        field["samples"] += 1
        field["correct"] += int(prediction["correct"])
    mandatory = {
        "current_score", "pgreat", "great", "good", "bad", "poor"
    }
    observations: dict[tuple[str, str, int], list[dict[str, Any]]] = {}
    for prediction in predictions:
        key = (
            prediction["session_sha256"],
            prediction["episode_id"],
            prediction["sequence"],
        )
        observations.setdefault(key, []).append(prediction)
        if prediction["field"] in {"current_score", "pgreat", "great"}:
            prediction["accepted"] = False
            prediction["accepted_text"] = None
    joint_rows = []
    for key, fields in observations.items():
        decision = _joint_score_decision({field["field"]: field for field in fields})
        if decision is not None:
            joint_rows.append((key, decision))
    joint_threshold = None
    for threshold in sorted(
        {0.0, *(decision["runner_up_margin"] or 0.0 for _, decision in joint_rows)}
    ):
        accepted = [
            decision for _, decision in joint_rows
            if decision["runner_up_margin"] is None
            or decision["runner_up_margin"] >= threshold
        ]
        incorrect = sum(not decision["correct"] for decision in accepted)
        correct = sum(decision["correct"] for decision in accepted)
        joint_candidate = {
            "minimum_runner_up_margin": threshold,
            "accepted_correct": correct,
            "accepted_incorrect": incorrect,
            "observations": len(joint_rows),
        }
        if incorrect == 0 and (
            joint_threshold is None
            or (correct, -threshold)
                > (joint_threshold["accepted_correct"], -joint_threshold["minimum_runner_up_margin"])
        ):
            joint_threshold = joint_candidate
    calibration["joint"] = joint_threshold
    temporal_joint_threshold = _select_zero_error_temporal_joint_calibration(joint_rows)
    if temporal_joint_threshold is None:
        raise NumericTrainingError("no zero-error temporal joint score calibration exists")
    temporal_calibration["joint"] = temporal_joint_threshold
    joint_by_key = {key: decision for key, decision in joint_rows}
    complete = 0
    accepted_complete = 0
    for fields in observations.values():
        by_name = {field["field"]: field for field in fields}
        complete += int(
            mandatory <= set(by_name)
            and all(by_name[field]["correct"] for field in mandatory)
        )
        key = (
            fields[0]["session_sha256"], fields[0]["episode_id"], fields[0]["sequence"]
        )
        joint = joint_by_key.get(key)
        joint_accepted = bool(
            joint is not None
            and (
                joint["runner_up_margin"] is None
                or joint["runner_up_margin"]
                    >= temporal_joint_threshold["minimum_runner_up_margin"]
            )
        )
        if joint is not None:
            for field in ("current_score", "pgreat", "great"):
                if field in by_name:
                    by_name[field]["accepted"] = joint_accepted
                    by_name[field]["accepted_text"] = (
                        str(joint["candidates"][0][field]) if joint_accepted else None
                    )
                    by_name[field]["joint_score"] = joint
        accepted_complete += int(
            mandatory <= set(by_name)
            and joint_accepted
            and bool(joint and joint["correct"])
            and all(
                by_name[field]["accepted"]
                and by_name[field]["accepted_text"] == by_name[field]["truth"]
                for field in mandatory - {"current_score", "pgreat", "great"}
            )
        )
    record = {
        "schema": EVALUATION_SCHEMA,
        "candidate": candidate,
        "dataset_sha256": dataset_sha256,
        "preparation_sha256": preparation_sha256,
        "training_manifest_sha256": training_sha256,
        "initializer_manifest_sha256": trained["initializer_manifest_sha256"],
        "initializer_checkpoint_sha256": trained["initializer_checkpoint_sha256"],
        "training_source_commit": trained["training_source_commit"],
        "training_recipe": trained["recipe"],
        "sample_count": len(predictions),
        "exact_count": sum(int(item["correct"]) for item in predictions),
        "incorrect_count": sum(int(not item["correct"]) for item in predictions),
        "observation_count": len(observations),
        "complete_mandatory_tuple_count": complete,
        "accepted_complete_mandatory_tuple_count": accepted_complete,
        "calibration": calibration,
        "temporal_calibration": temporal_calibration,
        "per_field": per_field,
        "predictions": predictions,
    }
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish(staging, output)
        return {key: value for key, value in record.items() if key != "predictions"}
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def train_final(
    candidate: str,
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    initializer: Path,
    initializer_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared(preparation, preparation_sha256)
    initialized = _initializer(initializer, initializer_sha256, candidate)
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        all_train_lines: set[str] = set()
        all_eval_lines: set[str] = set()
        for fold in range(prepared["folds"]):
            all_train_lines.update(
                (preparation / "folds" / f"fold-{fold}-train.txt")
                .read_text(encoding="utf-8").splitlines()
            )
            all_eval_lines.update(
                (preparation / "folds" / f"fold-{fold}-eval.txt")
                .read_text(encoding="utf-8").splitlines()
            )
        train_list = staging / "all-train.txt"
        eval_list = staging / "all-eval.txt"
        train_list.write_text("\n".join(sorted(all_train_lines)) + "\n", encoding="utf-8")
        eval_list.write_text("\n".join(sorted(all_eval_lines)) + "\n", encoding="utf-8")
        model_root = staging / "model"
        config = _training_config(
            candidate, source_root, preparation, initializer, 0, model_root
        )
        config["Train"]["dataset"]["label_file_list"] = [str(train_list)]
        config["Eval"]["dataset"]["label_file_list"] = [str(eval_list)]
        config_path = staging / "training-config.yml"
        config_path.write_text(
            yaml.safe_dump(config, allow_unicode=True, sort_keys=False), encoding="utf-8"
        )
        environment = os.environ.copy()
        environment["NO_ALBUMENTATIONS_UPDATE"] = "1"
        environment["OMP_NUM_THREADS"] = "8"
        run_checked(
            [sys.executable, str(source_root / source.training_entrypoint.path), "-c", str(config_path)],
            cwd=source_root,
            environment=environment,
            timeout_seconds=TRAINING_TIMEOUT_SECONDS,
        )
        selected = model_root / "best_accuracy.pdparams"
        if not selected.is_file():
            selected = model_root / "latest.pdparams"
        checkpoint = _hash_unpinned_file(selected, "numeric final checkpoint")
        shutil.copyfile(selected, staging / "selected.pdparams")
        shutil.rmtree(model_root)
        config_path.unlink()
        train_list.unlink()
        eval_list.unlink()
        record = {
            "schema": FINAL_TRAINING_SCHEMA,
            "candidate": candidate,
            "preparation_sha256": preparation_sha256,
            "initializer_manifest_sha256": initializer_sha256,
            "initializer_checkpoint_sha256": initialized["checkpoint"]["sha256"],
            "training_source_commit": source.commit,
            "source_samples": prepared["source_samples"],
            "prepared_samples": len(all_train_lines),
            "evaluation_samples": len(all_eval_lines),
            "recipe": {
                "epochs": TRAINING_EPOCHS,
                "batch_size": TRAINING_BATCH_SIZE,
                "learning_rate": TRAINING_LEARNING_RATE,
                "learning_rate_schedule": "constant",
                "device": "cpu",
                "seed": 0,
                "runtime_augmentation": False,
                "evaluation_step": TRAINING_EVALUATION_STEP,
            },
            "checkpoint": checkpoint,
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish_tree(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def export_final(
    candidate: str,
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    final: Path,
    final_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared(preparation, preparation_sha256)
    trained = _json(final / "manifest.json", MAX_MANIFEST_BYTES, final_sha256)
    source = load_registered_source()
    verify_source(source_root, source)
    if (
        trained.get("schema") != FINAL_TRAINING_SCHEMA
        or trained.get("candidate") != candidate
        or trained.get("preparation_sha256") != preparation_sha256
        or trained.get("training_source_commit") != source.commit
        or not isinstance(trained.get("recipe"), dict)
        or not isinstance(trained.get("checkpoint"), dict)
    ):
        raise NumericTrainingError("numeric final training manifest is invalid")
    checkpoint = trained["checkpoint"]
    checkpoint_bytes = _read_regular(
        final / "selected.pdparams", MAX_MODEL_FILE_BYTES, checkpoint.get("sha256")
    )
    if len(checkpoint_bytes) != checkpoint.get("bytes"):
        raise NumericTrainingError("numeric final checkpoint size mismatched")
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    with tempfile.TemporaryDirectory(prefix="scorepeek-numeric-export-") as temporary:
        work = Path(temporary)
        config = _training_config(candidate, source_root, preparation, final, 0, work / "unused")
        config["Global"]["pretrained_model"] = None
        config_path = work / "export-config.yml"
        config_path.write_text(
            yaml.safe_dump(config, allow_unicode=True, sort_keys=False), encoding="utf-8"
        )
        paddle_root = work / "paddle"
        environment = os.environ.copy()
        environment["NO_ALBUMENTATIONS_UPDATE"] = "1"
        run_checked(
            [
                sys.executable,
                str(source_root / source.export_entrypoint.path),
                "-c", str(config_path),
                "-o", f"Global.checkpoints={final / 'selected.pdparams'}",
                f"Global.save_inference_dir={paddle_root}",
                "Global.use_gpu=False",
            ],
            cwd=source_root,
            environment=environment,
            timeout_seconds=EXPORT_TIMEOUT_SECONDS,
        )
        onnx_path = work / "inference.onnx"
        run_checked(
            [
                shutil.which("paddle2onnx") or "paddle2onnx",
                "--model_dir", str(paddle_root),
                "--model_filename", "inference.json",
                "--params_filename", "inference.pdiparams",
                "--save_file", str(onnx_path),
                "--opset_version", str(ONNX_OPSET),
                "--enable_auto_update_opset", "False",
                "--enable_onnx_checker", "True",
                "--optimize_tool", "None",
            ],
            timeout_seconds=EXPORT_TIMEOUT_SECONDS,
        )
        model = onnx.load(onnx_path)
        if len(model.graph.input) != 1 or len(model.graph.output) != 1:
            raise NumericTrainingError("numeric ONNX tensor count is invalid")
        input_dimensions = model.graph.input[0].type.tensor_type.shape.dim
        output_dimensions = model.graph.output[0].type.tensor_type.shape.dim
        if (
            len(input_dimensions) != 4
            or len(output_dimensions) != 3
            or input_dimensions[1].dim_value != 3
            or input_dimensions[2].dim_value != 32
            or output_dimensions[2].dim_value != len(DICTIONARY) + 1
        ):
            raise NumericTrainingError("numeric ONNX tensor shape is invalid")
        input_dimensions[3].ClearField("dim_param")
        input_dimensions[3].dim_value = 320
        onnx.checker.check_model(model, full_check=True)
        onnx.save_model(model, onnx_path)
        staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
        try:
            filenames = {
                "paddle_graph": paddle_root / "inference.json",
                "paddle_parameters": paddle_root / "inference.pdiparams",
                "inference_config": paddle_root / "inference.yml",
                "onnx_model": onnx_path,
            }
            files = {}
            for role, source_path in filenames.items():
                destination = staging / source_path.name
                shutil.copyfile(source_path, destination)
                files[role] = {**_hash_unpinned_file(destination, role), "filename": destination.name}
            record = {
                "schema": EXPORT_SCHEMA,
                "candidate": candidate,
                "preparation_sha256": preparation_sha256,
                "final_training_manifest_sha256": final_sha256,
                "initializer_manifest_sha256": trained["initializer_manifest_sha256"],
                "initializer_checkpoint_sha256": trained["initializer_checkpoint_sha256"],
                "training_source_commit": trained["training_source_commit"],
                "training_recipe": trained["recipe"],
                "dictionary": DICTIONARY,
                "input_shape": [None, 3, 32, 320],
                "maximum_text_length": 4,
                "paddle2onnx_version": "2.1.0",
                "onnx_opset": ONNX_OPSET,
                "onnx_optimization": "none",
                "onnx_checker": True,
                "files": files,
            }
            (staging / "manifest.json").write_text(
                json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
                encoding="utf-8",
            )
            _publish_tree(staging, output)
            return record
        except BaseException:
            if staging.exists():
                shutil.rmtree(staging)
            raise


def _matching_training_lineage(exported: dict[str, Any], evaluated: dict[str, Any]) -> bool:
    pairs = (
        ("initializer_manifest_sha256", "initializer_manifest_sha256"),
        ("initializer_checkpoint_sha256", "initializer_checkpoint_sha256"),
        ("training_source_commit", "training_source_commit"),
        ("training_recipe", "training_recipe"),
    )
    return all(exported.get(left) == evaluated.get(right) for left, right in pairs)


def bundle_runtime(
    export: Path,
    export_sha256: str,
    evaluation: Path,
    evaluation_sha256: str,
    output: Path,
) -> dict[str, Any]:
    exported = _json(export / "manifest.json", MAX_MANIFEST_BYTES, export_sha256)
    evaluated = _json(
        evaluation / "manifest.json", MAX_EVALUATION_BYTES, evaluation_sha256
    )
    if (
        exported.get("schema") != EXPORT_SCHEMA
        or exported.get("candidate") != "mobile"
        or evaluated.get("schema") != EVALUATION_SCHEMA
        or evaluated.get("candidate") != "mobile"
        or exported.get("preparation_sha256") != evaluated.get("preparation_sha256")
        or not _matching_training_lineage(exported, evaluated)
        or not isinstance(exported.get("files"), dict)
        or not isinstance(evaluated.get("calibration"), dict)
        or not isinstance(evaluated.get("temporal_calibration"), dict)
        or evaluated.get("accepted_complete_mandatory_tuple_count", 0) <= 0
    ):
        raise NumericTrainingError("numeric runtime bundle inputs are invalid")
    model_record = exported["files"].get("onnx_model")
    graph_record = exported["files"].get("paddle_graph")
    parameters_record = exported["files"].get("paddle_parameters")
    if any(
        not isinstance(record, dict)
        or set(record) != {"sha256", "bytes", "filename"}
        for record in (model_record, graph_record, parameters_record)
    ):
        raise NumericTrainingError("numeric runtime export files are invalid")
    calibration = evaluated["temporal_calibration"]

    def runtime_calibration(family: str) -> dict[str, Any]:
        selected = calibration.get(family)
        if not isinstance(selected, dict):
            raise NumericTrainingError("numeric runtime calibration is missing")
        return {
            "enabled": selected["enabled"],
            "temperature": selected["temperature"],
            "minimum_probability": selected["minimum_probability"],
            "minimum_runner_up_margin": selected["minimum_runner_up_margin"],
        }

    joint = calibration.get("joint")
    if not isinstance(joint, dict):
        raise NumericTrainingError("numeric joint calibration is missing")
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        model_source = export / model_record["filename"]
        model_bytes = _read_regular(
            model_source, MAX_MODEL_FILE_BYTES, model_record["sha256"]
        )
        if len(model_bytes) != model_record["bytes"]:
            raise NumericTrainingError("numeric runtime ONNX size mismatched")
        (staging / "inference.onnx").write_bytes(model_bytes)
        record = {
            "schema": RUNTIME_SCHEMA,
            "model_id": f"scorepeek-numeric-mobile-ctc-{evaluated['dataset_sha256'][:8]}-v1",
            "model_filename": "inference.onnx",
            "model_sha256": model_record["sha256"],
            "model_bytes": model_record["bytes"],
            "candidate": "mobile",
            "dictionary": DICTIONARY,
            "preprocessor_id": "paddleocr-3.7.0-bgr-rec-resize-3x32x320-v1",
            "input_shape": [3, 32, 320],
            "output_classes": 12,
            "dataset_sha256": evaluated["dataset_sha256"],
            "preparation_sha256": evaluated["preparation_sha256"],
            "evaluation_manifest_sha256": evaluation_sha256,
            "final_training_manifest_sha256": exported["final_training_manifest_sha256"],
            "initializer_manifest_sha256": exported["initializer_manifest_sha256"],
            "initializer_checkpoint_sha256": exported["initializer_checkpoint_sha256"],
            "training_source_commit": exported["training_source_commit"],
            "export_manifest_sha256": export_sha256,
            "paddle_graph_sha256": graph_record["sha256"],
            "paddle_parameters_sha256": parameters_record["sha256"],
            "license_id": "Apache-2.0",
            "calibrations": {
                "level": runtime_calibration("level"),
                "notes": runtime_calibration("notes"),
                "score": runtime_calibration("score"),
                "judgment": runtime_calibration("judgment"),
                "supplemental": runtime_calibration("supplemental"),
                "joint_minimum_runner_up_margin": joint["minimum_runner_up_margin"],
            },
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish_tree(staging, output)
        return record
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def evaluate_sentinel(
    candidate: str,
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    final: Path,
    final_sha256: str,
    evaluation: Path,
    evaluation_sha256: str,
    sentinel: Path,
    sentinel_sha256: str,
    output: Path,
) -> dict[str, Any]:
    _prepared(preparation, preparation_sha256)
    trained = _json(final / "manifest.json", MAX_MANIFEST_BYTES, final_sha256)
    evaluated = _json(evaluation / "manifest.json", MAX_EVALUATION_BYTES, evaluation_sha256)
    sentinel_record = _json(sentinel / "manifest.json", MAX_DATASET_BYTES, sentinel_sha256)
    if (
        trained.get("schema") != FINAL_TRAINING_SCHEMA
        or trained.get("candidate") != candidate
        or evaluated.get("schema") != EVALUATION_SCHEMA
        or evaluated.get("candidate") != candidate
        or trained.get("preparation_sha256") != preparation_sha256
        or evaluated.get("preparation_sha256") != preparation_sha256
        or trained.get("initializer_manifest_sha256")
            != evaluated.get("initializer_manifest_sha256")
        or trained.get("initializer_checkpoint_sha256")
            != evaluated.get("initializer_checkpoint_sha256")
        or trained.get("training_source_commit")
            != evaluated.get("training_source_commit")
        or trained.get("recipe") != evaluated.get("training_recipe")
        or sentinel_record.get("schema")
            not in {
                "scorepeek-private-numeric-ctc-sentinel-v1",
                "scorepeek-private-numeric-ctc-sentinel-v2",
            }
        or sentinel_record.get("dictionary") != DICTIONARY
        or not isinstance(sentinel_record.get("samples"), list)
    ):
        raise NumericTrainingError("numeric sentinel inputs are invalid")
    sample_fields = [sample.get("field") for sample in sentinel_record["samples"]]
    required_fields = sentinel_record.get("required_fields", sample_fields)
    if (
        not sample_fields
        or any(field not in FIELD_FAMILIES for field in sample_fields)
        or len(set(sample_fields)) != len(sample_fields)
        or not {"current_score", "pgreat", "great"} <= set(sample_fields)
        or not isinstance(required_fields, list)
        or not required_fields
        or any(field not in FIELD_FAMILIES for field in required_fields)
        or len(set(required_fields)) != len(required_fields)
        or not set(required_fields) <= set(sample_fields)
    ):
        raise NumericTrainingError("numeric sentinel required fields are invalid")
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise NumericTrainingError("output must be a new absolute directory")
    sys.path.insert(0, str(source_root))
    from ppocr.modeling.architectures import build_model

    config = _training_config(candidate, source_root, preparation, final, 0, output)
    model = build_model(config["Architecture"])
    model.set_state_dict(paddle.load(str(final / "selected.pdparams")))
    model.eval()
    samples = sentinel_record["samples"]
    inputs = np.stack(
        [
            _preprocess_numeric(
                sentinel / sample["filename"], sample["crop_sha256"]
            )
            for sample in samples
        ]
    )
    with paddle.no_grad():
        probabilities = model(paddle.to_tensor(inputs)).numpy()
    calibration = evaluated.get("temporal_calibration", evaluated["calibration"])
    predictions = []
    for sample, probability in zip(samples, probabilities, strict=True):
        selected = calibration[FIELD_FAMILIES[sample["field"]]]
        inference = _exact_ctc_candidates(
            probability, sample["field"], selected["temperature"]
        )
        accepted = bool(
            inference["candidates"]
            and inference["candidates"][0]["log_probability"]
                > inference["all_blank_log_probability"]
            and inference["candidates"][0]["calibrated_probability"]
                >= selected["minimum_probability"]
            and (inference["runner_up_margin"] or 0.0)
                >= selected["minimum_runner_up_margin"]
        )
        predictions.append(
            {
                "field": sample["field"],
                "truth": sample["label"],
                "exact_ctc": inference,
                "accepted": accepted,
                "calibration_accepted": accepted,
                "accepted_text": inference["candidates"][0]["text"] if accepted else None,
            }
        )
    by_name = {prediction["field"]: prediction for prediction in predictions}
    joint = _joint_score_decision(by_name)
    joint_threshold = calibration["joint"]["minimum_runner_up_margin"]
    joint_accepted = bool(
        joint is not None
        and joint["correct"]
        and (joint["runner_up_margin"] is None or joint["runner_up_margin"] >= joint_threshold)
    )
    if joint is not None:
        for field in ("current_score", "pgreat", "great"):
            by_name[field]["accepted"] = joint_accepted
            by_name[field]["accepted_text"] = (
                str(joint["candidates"][0][field]) if joint_accepted else None
            )
    required_correct = all(
        by_name[field]["accepted"]
        and by_name[field]["accepted_text"] == by_name[field]["truth"]
        for field in required_fields
    )
    optional_wrong_accepts = [
        prediction["field"]
        for prediction in predictions
        if prediction["field"] not in required_fields
        and prediction["accepted"]
        and prediction["accepted_text"] != prediction["truth"]
    ]
    gate_passed = required_correct and not optional_wrong_accepts
    record = {
        "schema": SENTINEL_EVALUATION_SCHEMA,
        "candidate": candidate,
        "preparation_sha256": preparation_sha256,
        "final_training_manifest_sha256": final_sha256,
        "loso_evaluation_manifest_sha256": evaluation_sha256,
        "sentinel_manifest_sha256": sentinel_sha256,
        "input_tensor_sha256": _sha256(inputs.astype("<f4", copy=False).tobytes()),
        "paddle_output_tensor_sha256": _sha256(
            probabilities.astype("<f4", copy=False).tobytes()
        ),
        "joint_score": joint,
        "required_fields": required_fields,
        "all_required_correct": required_correct,
        "optional_wrong_accepts": optional_wrong_accepts,
        "gate_passed": gate_passed,
        "predictions": predictions,
    }
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish(staging, output)
        if not gate_passed:
            raise NumericTrainingError("numeric sentinel gate did not pass")
        return {key: value for key, value in record.items() if key != "predictions"}
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def subset_sentinel(
    source: Path,
    source_sha256: str,
    fields: str,
    required_fields: str,
    output: Path,
) -> dict[str, Any]:
    source_record = _json(source / "manifest.json", MAX_DATASET_BYTES, source_sha256)
    selected = fields.split(",")
    required = required_fields.split(",")
    if (
        source_record.get("schema") != "scorepeek-private-numeric-ctc-sentinel-v1"
        or source_record.get("dictionary") != DICTIONARY
        or not isinstance(source_record.get("samples"), list)
        or not selected
        or any(field not in FIELD_FAMILIES for field in selected)
        or len(set(selected)) != len(selected)
        or selected != [field for field in RUNTIME_FIELD_ORDER if field in set(selected)]
        or not required
        or len(set(required)) != len(required)
        or not set(required) <= set(selected)
        or required != [field for field in selected if field in set(required)]
        or not output.is_absolute()
        or output.exists()
        or not output.parent.is_dir()
    ):
        raise NumericTrainingError("numeric sentinel subset inputs are invalid")
    by_field = {sample.get("field"): sample for sample in source_record["samples"]}
    if any(field not in by_field for field in selected):
        raise NumericTrainingError("numeric sentinel subset field is missing")
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        images = staging / "images"
        images.mkdir(mode=0o700)
        samples = []
        for field in selected:
            sample = by_field[field]
            crop = _read_regular(
                source / sample["filename"], MAX_CROP_BYTES, sample["crop_sha256"]
            )
            filename = Path(sample["filename"]).name
            (images / filename).write_bytes(crop)
            samples.append({**sample, "filename": f"images/{filename}"})
        record = {
            "schema": "scorepeek-private-numeric-ctc-sentinel-v2",
            "sentinel_id": f"{source_record.get('sentinel_id', 'sentinel')}-visible-fields",
            "source_sentinel_sha256": source_sha256,
            "frame_sha256": source_record.get("frame_sha256"),
            "dictionary": DICTIONARY,
            "maximum_text_length": 4,
            "required_fields": required,
            "samples": samples,
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), allow_nan=False) + "\n",
            encoding="utf-8",
        )
        _publish_tree(staging, output)
        return {key: value for key, value in record.items() if key != "samples"}
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare_parser = subparsers.add_parser("prepare")
    prepare_parser.add_argument("--dataset", type=Path, required=True)
    prepare_parser.add_argument("--dataset-sha256", required=True)
    prepare_parser.add_argument("--output", type=Path, required=True)
    initialize_parser = subparsers.add_parser("initialize")
    initialize_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    initialize_parser.add_argument("--source", type=Path, required=True)
    initialize_parser.add_argument("--checkpoint", type=Path, required=True)
    initialize_parser.add_argument("--output", type=Path, required=True)
    train_parser = subparsers.add_parser("train-loso")
    train_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    train_parser.add_argument("--preparation", type=Path, required=True)
    train_parser.add_argument("--preparation-sha256", required=True)
    train_parser.add_argument("--source", type=Path, required=True)
    train_parser.add_argument("--initializer", type=Path, required=True)
    train_parser.add_argument("--initializer-sha256", required=True)
    train_parser.add_argument("--output", type=Path, required=True)
    final_parser = subparsers.add_parser("train-final")
    final_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    final_parser.add_argument("--preparation", type=Path, required=True)
    final_parser.add_argument("--preparation-sha256", required=True)
    final_parser.add_argument("--source", type=Path, required=True)
    final_parser.add_argument("--initializer", type=Path, required=True)
    final_parser.add_argument("--initializer-sha256", required=True)
    final_parser.add_argument("--output", type=Path, required=True)
    evaluate_parser = subparsers.add_parser("evaluate-loso")
    evaluate_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    evaluate_parser.add_argument("--dataset", type=Path, required=True)
    evaluate_parser.add_argument("--dataset-sha256", required=True)
    evaluate_parser.add_argument("--preparation", type=Path, required=True)
    evaluate_parser.add_argument("--preparation-sha256", required=True)
    evaluate_parser.add_argument("--source", type=Path, required=True)
    evaluate_parser.add_argument("--training", type=Path, required=True)
    evaluate_parser.add_argument("--training-sha256", required=True)
    evaluate_parser.add_argument("--output", type=Path, required=True)
    export_parser = subparsers.add_parser("export-final")
    export_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    export_parser.add_argument("--preparation", type=Path, required=True)
    export_parser.add_argument("--preparation-sha256", required=True)
    export_parser.add_argument("--source", type=Path, required=True)
    export_parser.add_argument("--final", type=Path, required=True)
    export_parser.add_argument("--final-sha256", required=True)
    export_parser.add_argument("--output", type=Path, required=True)
    bundle_parser = subparsers.add_parser("bundle-runtime")
    bundle_parser.add_argument("--export", type=Path, required=True)
    bundle_parser.add_argument("--export-sha256", required=True)
    bundle_parser.add_argument("--evaluation", type=Path, required=True)
    bundle_parser.add_argument("--evaluation-sha256", required=True)
    bundle_parser.add_argument("--output", type=Path, required=True)
    sentinel_parser = subparsers.add_parser("evaluate-sentinel")
    sentinel_parser.add_argument("--candidate", choices=("mobile", "ppocrv6"), required=True)
    sentinel_parser.add_argument("--preparation", type=Path, required=True)
    sentinel_parser.add_argument("--preparation-sha256", required=True)
    sentinel_parser.add_argument("--source", type=Path, required=True)
    sentinel_parser.add_argument("--final", type=Path, required=True)
    sentinel_parser.add_argument("--final-sha256", required=True)
    sentinel_parser.add_argument("--evaluation", type=Path, required=True)
    sentinel_parser.add_argument("--evaluation-sha256", required=True)
    sentinel_parser.add_argument("--sentinel", type=Path, required=True)
    sentinel_parser.add_argument("--sentinel-sha256", required=True)
    sentinel_parser.add_argument("--output", type=Path, required=True)
    subset_parser = subparsers.add_parser("subset-sentinel")
    subset_parser.add_argument("--source", type=Path, required=True)
    subset_parser.add_argument("--source-sha256", required=True)
    subset_parser.add_argument("--fields", required=True)
    subset_parser.add_argument("--required-fields", required=True)
    subset_parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "prepare":
            result = prepare(
                arguments.dataset, arguments.dataset_sha256, arguments.output
            )
        elif arguments.command == "initialize":
            result = initialize(
                arguments.candidate, arguments.source, arguments.checkpoint, arguments.output
            )
        elif arguments.command == "train-loso":
            result = train_loso(
                arguments.candidate,
                arguments.preparation,
                arguments.preparation_sha256,
                arguments.source,
                arguments.initializer,
                arguments.initializer_sha256,
                arguments.output,
            )
        elif arguments.command == "train-final":
            result = train_final(
                arguments.candidate,
                arguments.preparation,
                arguments.preparation_sha256,
                arguments.source,
                arguments.initializer,
                arguments.initializer_sha256,
                arguments.output,
            )
        elif arguments.command == "evaluate-loso":
            result = evaluate_loso(
                arguments.candidate,
                arguments.dataset,
                arguments.dataset_sha256,
                arguments.preparation,
                arguments.preparation_sha256,
                arguments.source,
                arguments.training,
                arguments.training_sha256,
                arguments.output,
            )
        elif arguments.command == "export-final":
            result = export_final(
                arguments.candidate,
                arguments.preparation,
                arguments.preparation_sha256,
                arguments.source,
                arguments.final,
                arguments.final_sha256,
                arguments.output,
            )
        elif arguments.command == "bundle-runtime":
            result = bundle_runtime(
                arguments.export,
                arguments.export_sha256,
                arguments.evaluation,
                arguments.evaluation_sha256,
                arguments.output,
            )
        elif arguments.command == "evaluate-sentinel":
            result = evaluate_sentinel(
                arguments.candidate,
                arguments.preparation,
                arguments.preparation_sha256,
                arguments.source,
                arguments.final,
                arguments.final_sha256,
                arguments.evaluation,
                arguments.evaluation_sha256,
                arguments.sentinel,
                arguments.sentinel_sha256,
                arguments.output,
            )
        else:
            result = subset_sentinel(
                arguments.source,
                arguments.source_sha256,
                arguments.fields,
                arguments.required_fields,
                arguments.output,
            )
    except Exception as error:
        print(f"scorepeek numeric training failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
