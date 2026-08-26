"""Content-addressed acquisition for registered offline OCR models."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.request
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MAX_MANIFEST_BYTES = 64 * 1024
MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_ONNX_BUNDLE_FILE_BYTES = 128 * 1024 * 1024
MAX_ONNX_BUNDLE_COUNT = 8
MAX_ONNX_BUNDLE_BYTES = 512 * 1024 * 1024
MAX_ONNX_BUNDLE_OBJECT_BYTES = 192 * 1024 * 1024
ONNX_BUNDLE_LOCK = ".writer.lock"
ONNX_BUNDLE_STORE_CLAIM = ".scorepeek-onnx-bundle-store-claim-v1"
ONNX_BUNDLE_STORE_MARKER = ".scorepeek-onnx-bundle-store-v1"
ONNX_BUNDLE_STAGING_PREFIX = ".scorepeek-staging-"
ONNX_BUNDLE_STAGING_MARKER = ".scorepeek-onnx-bundle-staging-v1"
PROJECT_ROOT = Path(__file__).resolve().parents[2]
REGISTERED_MODEL_MANIFEST = (
    PROJECT_ROOT / "models" / "manifests" / "pp-ocrv6-small-rec-v1.json"
)
REGISTERED_MODEL_MANIFEST_SHA256 = (
    "ccb361d69880cf98cb61a50bfbf9f6c5e46d76d6b0c93eed53ee9b99ec4d8ab8"
)
REGISTERED_ONNX_MODEL_MANIFEST = (
    PROJECT_ROOT / "models" / "manifests" / "pp-ocrv6-small-rec-onnx-v1.json"
)
REGISTERED_ONNX_MODEL_MANIFEST_SHA256 = (
    "48cc68b16e785c4b2a0fa2a7764bb1ac6e87e9199065f5bea090a94fca97ee6c"
)
REGISTERED_ONNX_BUNDLE_MANIFESTS = {
    "pp-ocrv6-small-rec-onnx-v1": (
        PROJECT_ROOT
        / "models"
        / "manifests"
        / "pp-ocrv6-small-rec-onnx-bundle-v1.json",
        "4064dfa4124ada63613fe39fe2dee92f6ce6cae898e2830b302f5ae593f60672",
    ),
    "pp-ocrv6-tiny-rec-onnx-v1": (
        PROJECT_ROOT
        / "models"
        / "manifests"
        / "pp-ocrv6-tiny-rec-onnx-bundle-v1.json",
        "d24f1ec10098065efd24216b23b405bb2af5feabbb815bc499ba0a5735b8bfd0",
    ),
    "pp-ocrv6-medium-rec-onnx-v1": (
        PROJECT_ROOT
        / "models"
        / "manifests"
        / "pp-ocrv6-medium-rec-onnx-bundle-v1.json",
        "f794d77fb6d9860e2aadedd1ef575bd67c044b83fe2821243867b66c9a7c5abe",
    ),
    "pp-ocrv5-mobile-rec-onnx-v1": (
        PROJECT_ROOT
        / "models"
        / "manifests"
        / "pp-ocrv5-mobile-rec-onnx-bundle-v1.json",
        "ebbd34d2c0e360b1cf55199fc1400886e7bfbb4d6917c7d86a994b79c2256971",
    ),
    "pp-ocrv5-server-rec-onnx-v1": (
        PROJECT_ROOT
        / "models"
        / "manifests"
        / "pp-ocrv5-server-rec-onnx-bundle-v1.json",
        "4fe22f41508ed31b86e86caa88d433a20702d0a6e95cea07bcaca577441594fe",
    ),
}
REGISTERED_ONNX_BUNDLE_CONTRACTS = {
    "pp-ocrv6-small-rec-onnx-v1": {
        "model_name": "PP-OCRv6_small_rec",
        "repository": "PaddlePaddle/PP-OCRv6_small_rec_onnx",
        "revision": "b8f84f0b80c529de40b4fbb3544b84fa7233a513",
        "output_classes": 18710,
        "files": {
            "inference.onnx": (
                "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
                21_159_378,
            ),
            "inference.json": (
                "f0bf53c853937a917affdd74467472167727f8ab0f0f7bded01c4a16c27e46e6",
                208_004,
            ),
            "inference.yml": (
                "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
                150_579,
            ),
        },
    },
    "pp-ocrv6-tiny-rec-onnx-v1": {
        "model_name": "PP-OCRv6_tiny_rec",
        "repository": "PaddlePaddle/PP-OCRv6_tiny_rec_onnx",
        "revision": "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30",
        "output_classes": 6906,
        "files": {
            "inference.onnx": (
                "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6",
                4_462_639,
            ),
            "inference.json": (
                "b5b14770c7dcf092781e92f4278a2ae5f95048f08b4b8a04140e88cb2745f147",
                108_959,
            ),
            "inference.yml": (
                "66170210bad538e83fff3c4a3867e547d6bf20b50d64b20347c4b913f3034ea1",
                55_571,
            ),
        },
    },
    "pp-ocrv6-medium-rec-onnx-v1": {
        "model_name": "PP-OCRv6_medium_rec",
        "repository": "PaddlePaddle/PP-OCRv6_medium_rec_onnx",
        "revision": "50c7eacafc52fa7bcf4194e8cd08e46f8558504b",
        "output_classes": 18710,
        "files": {
            "inference.onnx": (
                "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
                76_554_979,
            ),
            "inference.json": (
                "0b2e25e990bd072f1bf77d59d67d508bce6c4bd44af6624e0fb27d6da2cd00e8",
                221_814,
            ),
            "inference.yml": (
                "991b700facf5b50a7de193468207d5f4255b538dde0d312ae3b7c7a9b6873129",
                150_580,
            ),
        },
    },
    "pp-ocrv5-mobile-rec-onnx-v1": {
        "model_name": "PP-OCRv5_mobile_rec",
        "repository": "PaddlePaddle/PP-OCRv5_mobile_rec_onnx",
        "revision": "ed152b8b495f84de93cda5709d768548a9127622",
        "output_classes": 18385,
        "files": {
            "inference.onnx": (
                "da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092",
                16_534_782,
            ),
            "inference.yml": (
                "5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e",
                148_345,
            ),
        },
    },
    "pp-ocrv5-server-rec-onnx-v1": {
        "model_name": "PP-OCRv5_server_rec",
        "repository": "PaddlePaddle/PP-OCRv5_server_rec_onnx",
        "revision": "b70df217f4fd99d14f970bad092cebe7d74cc4d1",
        "output_classes": 18385,
        "files": {
            "inference.onnx": (
                "d9dc333c9c7b042c6dffb8e33d72b6f65c9c1d463d0a3c2f78174fea55e94752",
                84_503_027,
            ),
            "inference.yml": (
                "2c719dba044c4e2228aef8ff92f5f575394d75d24c16de096a33b7cfd902f66d",
                148_345,
            ),
        },
    },
}


class ModelStoreError(Exception):
    """The registered model could not be acquired or verified."""


@dataclass(frozen=True)
class ModelFile:
    archive_path: str
    filename: str
    sha256: str
    bytes: int


@dataclass(frozen=True)
class ModelSource:
    model_id: str
    model_name: str
    source_url: str
    archive_sha256: str
    archive_bytes: int
    paddleocr_version: str
    paddlepaddle_version: str
    files: tuple[ModelFile, ...]


@dataclass(frozen=True)
class OnnxModelSource:
    model_id: str
    model_name: str
    source_url: str
    sha256: str
    bytes: int
    paddle_model_id: str
    paddle_inference_json_sha256: str
    paddle_inference_yml_sha256: str


@dataclass(frozen=True)
class OnnxBundleFile:
    filename: str
    source_url: str
    sha256: str
    bytes: int


@dataclass(frozen=True)
class OnnxNativeContract:
    input_layout: str
    input_color_order: str
    input_channels: int
    input_height: int
    preprocessor_minimum_width: int
    preprocessor_maximum_width: int
    output_classes: int
    ctc_blank_token: int


@dataclass(frozen=True)
class OnnxBundleSource:
    manifest_sha256: str
    model_id: str
    model_name: str
    source_repository: str
    source_revision: str
    native_contract: OnnxNativeContract
    files: tuple[OnnxBundleFile, ...]


def _sha256_file(path: Path, maximum: int) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    with _open_regular(path) as source:
        before = os.fstat(source.fileno())
        while chunk := source.read(1024 * 1024):
            total += len(chunk)
            if total > maximum:
                raise ModelStoreError(f"file exceeds the registered bound: {path}")
            digest.update(chunk)
        after = os.fstat(source.fileno())
    if _file_identity(before) != _file_identity(after) or total != before.st_size:
        raise ModelStoreError(f"file changed while reading: {path}")
    return digest.hexdigest(), total


def _open_regular(path: Path):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode):
        os.close(descriptor)
        raise ModelStoreError(f"not a regular file: {path}")
    return os.fdopen(descriptor, "rb")


def _regular_size(path: Path, maximum: int) -> int:
    with _open_regular(path) as source:
        size = os.fstat(source.fileno()).st_size
    if size <= 0 or size > maximum:
        raise ModelStoreError(f"file exceeds the registered bound: {path}")
    return size


def _file_identity(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
    )


def _fsync_directory(path: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISDIR(metadata.st_mode):
            raise ModelStoreError(f"not a directory: {path}")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _ensure_durable_directory(path: Path) -> None:
    missing = []
    current = path
    while not current.exists():
        missing.append(current)
        current = current.parent
    if current.is_symlink() or not current.is_dir():
        raise ModelStoreError(f"managed directory parent is invalid: {current}")
    for directory in reversed(missing):
        try:
            directory.mkdir(mode=0o700)
        except FileExistsError:
            pass
        if directory.is_symlink() or not directory.is_dir():
            raise ModelStoreError(f"managed directory is invalid: {directory}")
        _fsync_directory(directory)
        _fsync_directory(directory.parent)


@contextmanager
def _onnx_bundle_writer_lock(store: Path):
    lock_path = store / ONNX_BUNDLE_LOCK
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(lock_path, flags, 0o600)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise ModelStoreError("ONNX bundle writer lock is invalid")
        os.fsync(descriptor)
        _fsync_directory(store)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def _recover_onnx_bundle_staging(bundles: Path) -> None:
    changed = False
    for entry in bundles.iterdir():
        if entry.name.startswith(ONNX_BUNDLE_STAGING_PREFIX):
            if not entry.is_symlink() and entry.is_dir():
                shutil.rmtree(entry)
                changed = True
            continue
        if not _valid_sha256(entry.name):
            continue
        marker = entry / ONNX_BUNDLE_STAGING_MARKER
        if entry.is_symlink() or not entry.is_dir() or marker.is_symlink():
            continue
        try:
            marker_bytes = _read_regular_bytes(marker, 128)
        except (FileNotFoundError, ModelStoreError):
            continue
        if marker_bytes != b"scorepeek-owned-onnx-bundle-staging-v1\n":
            continue
        shutil.rmtree(entry)
        changed = True
    if changed:
        _fsync_directory(bundles)


def _write_marker(path: Path, expected: bytes, *, exclusive: bool) -> None:
    flags = os.O_WRONLY | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    flags |= os.O_EXCL if exclusive else os.O_TRUNC
    descriptor = os.open(path, flags, 0o600)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise ModelStoreError("ONNX bundle ownership marker is invalid")
        if os.write(descriptor, expected) != len(expected):
            raise ModelStoreError("ONNX bundle ownership marker write was incomplete")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    _fsync_directory(path.parent)


def _marker_matches(path: Path, expected: bytes) -> bool:
    try:
        return _read_regular_bytes(path, len(expected)) == expected
    except FileNotFoundError:
        return False


def _ensure_onnx_bundle_store(store: Path) -> Path:
    bundles = store / "bundles"
    claim = store / ONNX_BUNDLE_STORE_CLAIM
    marker = bundles / ONNX_BUNDLE_STORE_MARKER
    marker_bytes = b"scorepeek-owned-onnx-bundle-store-v1\n"
    claim_exists = claim.exists()
    if claim_exists and (
        claim.is_symlink() or not claim.is_dir() or any(claim.iterdir())
    ):
        raise ModelStoreError("ONNX bundle store claim is invalid")
    if bundles.exists():
        if bundles.is_symlink() or not bundles.is_dir():
            raise ModelStoreError("ONNX bundle store must be a directory")
        if _marker_matches(marker, marker_bytes):
            return bundles
        if not claim_exists:
            raise ModelStoreError("existing ONNX bundle store is not scorepeek-owned")
        unexpected = {entry.name for entry in bundles.iterdir()} - {
            ONNX_BUNDLE_STORE_MARKER
        }
        if unexpected:
            raise ModelStoreError("incomplete ONNX bundle store contains data")
        _write_marker(marker, marker_bytes, exclusive=not marker.exists())
        return bundles
    if not claim_exists:
        claim.mkdir(mode=0o700)
        _fsync_directory(claim)
        _fsync_directory(store)
    _ensure_durable_directory(bundles)
    _write_marker(marker, marker_bytes, exclusive=True)
    return bundles


def _onnx_bundle_store_usage(bundles: Path) -> tuple[int, int]:
    count = 0
    total = 0
    for entry in bundles.iterdir():
        if entry.name in {ONNX_BUNDLE_LOCK, ONNX_BUNDLE_STORE_MARKER}:
            continue
        if (
            entry.name.startswith(ONNX_BUNDLE_STAGING_PREFIX)
            or not _valid_sha256(entry.name)
            or entry.is_symlink()
            or not entry.is_dir()
        ):
            raise ModelStoreError("ONNX bundle store contains an unmanaged entry")
        object_bytes = 0
        file_count = 0
        for item in entry.iterdir():
            size = _regular_size(item, MAX_ONNX_BUNDLE_FILE_BYTES)
            file_count += 1
            object_bytes += size
            if file_count > 8 or object_bytes > MAX_ONNX_BUNDLE_OBJECT_BYTES:
                raise ModelStoreError("ONNX bundle object exceeds its byte bound")
        if file_count == 0:
            raise ModelStoreError("ONNX bundle object is empty")
        count += 1
        total += object_bytes
        if count > MAX_ONNX_BUNDLE_COUNT or total > MAX_ONNX_BUNDLE_BYTES:
            raise ModelStoreError("ONNX bundle store exceeds its capacity")
    return count, total


def _read_regular_bytes(path: Path, maximum: int) -> bytes:
    with _open_regular(path) as source:
        before = os.fstat(source.fileno())
        if before.st_size <= 0 or before.st_size > maximum:
            raise ModelStoreError(f"file exceeds the registered bound: {path}")
        data = source.read(maximum + 1)
        after = os.fstat(source.fileno())
    if (
        _file_identity(before) != _file_identity(after)
        or len(data) != before.st_size
        or len(data) > maximum
    ):
        raise ModelStoreError(f"file changed while reading: {path}")
    return data


def _exact_object(value: Any, keys: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise ModelStoreError(f"invalid {context} fields")
    return value


def load_source(path: Path) -> ModelSource:
    return _load_source_bytes(_read_regular_bytes(path, MAX_MANIFEST_BYTES))


def _load_source_bytes(data: bytes) -> ModelSource:
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise ModelStoreError("model source manifest is invalid") from error
    raw = _exact_object(
        raw,
        {
            "schema",
            "model_id",
            "model_name",
            "source_url",
            "archive_sha256",
            "archive_bytes",
            "license_id",
            "license_url",
            "paddleocr_version",
            "paddlepaddle_version",
            "files",
        },
        "model source manifest",
    )
    if (
        raw["schema"] != "scorepeek-ocr-model-source-v1"
        or raw["model_id"] != "pp-ocrv6-small-rec-v1"
        or raw["model_name"] != "PP-OCRv6_small_rec"
        or raw["license_id"] != "Apache-2.0"
        or not isinstance(raw["source_url"], str)
        or not raw["source_url"].startswith("https://paddle-model-ecology.bj.bcebos.com/")
        or not isinstance(raw["license_url"], str)
        or not _valid_sha256(raw["archive_sha256"])
        or not isinstance(raw["archive_bytes"], int)
        or not 0 < raw["archive_bytes"] <= MAX_ARCHIVE_BYTES
        or raw["paddleocr_version"] != "3.7.0"
        or raw["paddlepaddle_version"] != "3.3.1"
        or not isinstance(raw["files"], list)
        or len(raw["files"]) != 3
    ):
        raise ModelStoreError("model source manifest values are invalid")
    files = []
    for entry in raw["files"]:
        entry = _exact_object(
            entry, {"archive_path", "filename", "sha256", "bytes"}, "model file"
        )
        if (
            not isinstance(entry["archive_path"], str)
            or not entry["archive_path"].startswith("PP-OCRv6_small_rec_infer/")
            or not isinstance(entry["filename"], str)
            or Path(entry["filename"]).name != entry["filename"]
            or not _valid_sha256(entry["sha256"])
            or not isinstance(entry["bytes"], int)
            or not 0 < entry["bytes"] <= MAX_ARCHIVE_BYTES
        ):
            raise ModelStoreError("model file values are invalid")
        files.append(ModelFile(**entry))
    if {item.filename for item in files} != {
        "inference.json",
        "inference.pdiparams",
        "inference.yml",
    }:
        raise ModelStoreError("model source file set is invalid")
    return ModelSource(
        model_id=raw["model_id"],
        model_name=raw["model_name"],
        source_url=raw["source_url"],
        archive_sha256=raw["archive_sha256"],
        archive_bytes=raw["archive_bytes"],
        paddleocr_version=raw["paddleocr_version"],
        paddlepaddle_version=raw["paddlepaddle_version"],
        files=tuple(files),
    )


def load_registered_source() -> ModelSource:
    data = _read_regular_bytes(REGISTERED_MODEL_MANIFEST, MAX_MANIFEST_BYTES)
    if hashlib.sha256(data).hexdigest() != REGISTERED_MODEL_MANIFEST_SHA256:
        raise ModelStoreError("registered model manifest digest mismatch")
    return _load_source_bytes(data)


def load_registered_onnx_source() -> OnnxModelSource:
    data = _read_regular_bytes(REGISTERED_ONNX_MODEL_MANIFEST, MAX_MANIFEST_BYTES)
    if hashlib.sha256(data).hexdigest() != REGISTERED_ONNX_MODEL_MANIFEST_SHA256:
        raise ModelStoreError("registered ONNX model manifest digest mismatch")
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise ModelStoreError("ONNX model source manifest is invalid") from error
    raw = _exact_object(
        raw,
        {
            "schema",
            "model_id",
            "model_name",
            "source_repository",
            "source_revision",
            "source_url",
            "sha256",
            "bytes",
            "license_id",
            "license_url",
            "paddle_model_id",
            "paddle_inference_json_sha256",
            "paddle_inference_yml_sha256",
        },
        "ONNX model source manifest",
    )
    revision = "3d2d345e6a299891174f1397a72cdd81331359c7"
    expected_url = (
        "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/"
        f"{revision}/inference.onnx"
    )
    if (
        raw["schema"] != "scorepeek-ocr-onnx-model-source-v1"
        or raw["model_id"] != "pp-ocrv6-small-rec-onnx-v1"
        or raw["model_name"] != "PP-OCRv6_small_rec"
        or raw["source_repository"]
        != "PaddlePaddle/PP-OCRv6_small_rec_onnx"
        or raw["source_revision"] != revision
        or raw["source_url"] != expected_url
        or raw["sha256"]
        != "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634"
        or raw["bytes"] != 21_159_378
        or raw["license_id"] != "Apache-2.0"
        or not isinstance(raw["license_url"], str)
        or raw["paddle_model_id"] != "pp-ocrv6-small-rec-v1"
        or raw["paddle_inference_json_sha256"]
        != "f0bf53c853937a917affdd74467472167727f8ab0f0f7bded01c4a16c27e46e6"
        or raw["paddle_inference_yml_sha256"]
        != "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1"
    ):
        raise ModelStoreError("ONNX model source manifest values are invalid")
    return OnnxModelSource(
        model_id=raw["model_id"],
        model_name=raw["model_name"],
        source_url=raw["source_url"],
        sha256=raw["sha256"],
        bytes=raw["bytes"],
        paddle_model_id=raw["paddle_model_id"],
        paddle_inference_json_sha256=raw["paddle_inference_json_sha256"],
        paddle_inference_yml_sha256=raw["paddle_inference_yml_sha256"],
    )


def load_registered_onnx_bundle(model_id: str) -> OnnxBundleSource:
    registration = REGISTERED_ONNX_BUNDLE_MANIFESTS.get(model_id)
    if registration is None:
        raise ModelStoreError("ONNX bundle model ID is not registered")
    path, expected_manifest_sha256 = registration
    expected_contract = REGISTERED_ONNX_BUNDLE_CONTRACTS[model_id]
    data = _read_regular_bytes(path, MAX_MANIFEST_BYTES)
    if hashlib.sha256(data).hexdigest() != expected_manifest_sha256:
        raise ModelStoreError("registered ONNX bundle manifest digest mismatch")
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise ModelStoreError("ONNX bundle manifest is invalid") from error
    raw = _exact_object(
        raw,
        {
            "schema",
            "model_id",
            "model_name",
            "source_repository",
            "source_revision",
            "license_id",
            "license_url",
            "native_contract",
            "files",
        },
        "ONNX bundle manifest",
    )
    revision = expected_contract["revision"]
    repository = expected_contract["repository"]
    contract_raw = _exact_object(
        raw["native_contract"],
        {
            "input_layout",
            "input_color_order",
            "input_channels",
            "input_height",
            "preprocessor_minimum_width",
            "preprocessor_maximum_width",
            "output_classes",
            "ctc_blank_token",
        },
        "ONNX native contract",
    )
    if (
        raw["schema"] != "scorepeek-ocr-onnx-model-bundle-v1"
        or raw["model_id"] != model_id
        or raw["model_name"] != expected_contract["model_name"]
        or raw["source_repository"] != repository
        or raw["source_revision"] != revision
        or raw["license_id"] != "Apache-2.0"
        or raw["license_url"]
        != f"https://huggingface.co/{repository}/blob/{revision}/README.md"
        or contract_raw
        != {
            "input_layout": "NCHW",
            "input_color_order": "BGR",
            "input_channels": 3,
            "input_height": 48,
            "preprocessor_minimum_width": 320,
            "preprocessor_maximum_width": 3200,
            "output_classes": expected_contract["output_classes"],
            "ctc_blank_token": 0,
        }
        or not isinstance(raw["files"], list)
        or len(raw["files"]) != len(expected_contract["files"])
    ):
        raise ModelStoreError("ONNX bundle manifest values are invalid")
    expected_files = expected_contract["files"]
    files = []
    for entry in raw["files"]:
        entry = _exact_object(
            entry, {"filename", "source_url", "sha256", "bytes"}, "ONNX bundle file"
        )
        expected = expected_files.get(entry["filename"])
        expected_url = (
            f"https://huggingface.co/{repository}/resolve/{revision}/{entry['filename']}"
        )
        if (
            expected is None
            or Path(entry["filename"]).name != entry["filename"]
            or entry["source_url"] != expected_url
            or (entry["sha256"], entry["bytes"]) != expected
            or entry["bytes"] > MAX_ONNX_BUNDLE_FILE_BYTES
        ):
            raise ModelStoreError("ONNX bundle file values are invalid")
        files.append(OnnxBundleFile(**entry))
    if {item.filename for item in files} != set(expected_files):
        raise ModelStoreError("ONNX bundle file set is invalid")
    return OnnxBundleSource(
        manifest_sha256=expected_manifest_sha256,
        model_id=raw["model_id"],
        model_name=raw["model_name"],
        source_repository=raw["source_repository"],
        source_revision=raw["source_revision"],
        native_contract=OnnxNativeContract(**contract_raw),
        files=tuple(files),
    )


def _valid_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def default_store() -> Path:
    configured = os.environ.get("XDG_CACHE_HOME")
    if configured:
        base = Path(configured)
    else:
        home = os.environ.get("HOME")
        if not home:
            raise ModelStoreError("HOME is required when XDG_CACHE_HOME is unset")
        base = Path(home) / ".cache"
    if not base.is_absolute():
        raise ModelStoreError("model store base must be absolute")
    return base / "scorepeek" / "models"


def model_path(store: Path, source: ModelSource) -> Path:
    if not store.is_absolute():
        raise ModelStoreError("model store must be absolute")
    return store / "objects" / source.archive_sha256


def onnx_model_path(store: Path, source: OnnxModelSource) -> Path:
    if not store.is_absolute():
        raise ModelStoreError("model store must be absolute")
    return store / "objects" / source.sha256 / "inference.onnx"


def onnx_bundle_path(store: Path, source: OnnxBundleSource) -> Path:
    if not store.is_absolute():
        raise ModelStoreError("model store must be absolute")
    return store / "bundles" / source.manifest_sha256


def read_verified_model_files(
    directory: Path, source: ModelSource
) -> dict[str, bytes]:
    if directory.is_symlink() or not directory.is_dir():
        raise ModelStoreError("registered model object is not a directory")
    if {entry.name for entry in directory.iterdir()} != {
        item.filename for item in source.files
    }:
        raise ModelStoreError("registered model object has an unexpected file set")
    files = {}
    for item in source.files:
        data = _read_regular_bytes(directory / item.filename, item.bytes)
        if len(data) != item.bytes or hashlib.sha256(data).hexdigest() != item.sha256:
            raise ModelStoreError(f"registered model file mismatch: {item.filename}")
        files[item.filename] = data
    return files


def verify_model(directory: Path, source: ModelSource) -> None:
    read_verified_model_files(directory, source)


def verify_onnx_model(path: Path, source: OnnxModelSource) -> None:
    if path.parent.is_symlink() or not path.parent.is_dir():
        raise ModelStoreError("registered ONNX model object directory is invalid")
    if path.is_symlink() or not path.is_file():
        raise ModelStoreError("registered ONNX model object is not a regular file")
    if {entry.name for entry in path.parent.iterdir()} != {"inference.onnx"}:
        raise ModelStoreError("registered ONNX model object has an unexpected file set")
    digest, size = _sha256_file(path, source.bytes)
    if digest != source.sha256 or size != source.bytes:
        raise ModelStoreError("registered ONNX model file mismatch")


def verify_onnx_bundle(
    directory: Path, source: OnnxBundleSource, *, staging: bool = False
) -> None:
    if directory.is_symlink() or not directory.is_dir():
        raise ModelStoreError("registered ONNX bundle is not a directory")
    expected = {item.filename for item in source.files}
    if staging:
        expected.add(ONNX_BUNDLE_STAGING_MARKER)
    if {entry.name for entry in directory.iterdir()} != expected:
        raise ModelStoreError("registered ONNX bundle has an unexpected file set")
    for item in source.files:
        path = directory / item.filename
        digest, size = _sha256_file(path, item.bytes)
        if digest != item.sha256 or size != item.bytes:
            raise ModelStoreError(f"registered ONNX bundle file mismatch: {item.filename}")


def _download(source: ModelSource, path: Path) -> None:
    request = urllib.request.Request(source.source_url, headers={"User-Agent": "scorepeek/0"})
    digest = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(request, timeout=30) as response, path.open("xb") as output:
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > source.archive_bytes:
                raise ModelStoreError("model archive exceeds the registered size")
            digest.update(chunk)
            output.write(chunk)
    if total != source.archive_bytes or digest.hexdigest() != source.archive_sha256:
        raise ModelStoreError("model archive digest or size mismatch")


def _download_onnx(source: OnnxModelSource, path: Path) -> None:
    request = urllib.request.Request(source.source_url, headers={"User-Agent": "scorepeek/0"})
    digest = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(request, timeout=30) as response, path.open("xb") as output:
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > source.bytes:
                raise ModelStoreError("ONNX model exceeds the registered size")
            digest.update(chunk)
            output.write(chunk)
    if total != source.bytes or digest.hexdigest() != source.sha256:
        raise ModelStoreError("ONNX model digest or size mismatch")


def _download_onnx_bundle_file(source: OnnxBundleFile, path: Path) -> None:
    request = urllib.request.Request(source.source_url, headers={"User-Agent": "scorepeek/0"})
    digest = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(request, timeout=30) as response, path.open("xb") as output:
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > source.bytes:
                raise ModelStoreError("ONNX bundle file exceeds the registered size")
            digest.update(chunk)
            output.write(chunk)
        output.flush()
        os.fsync(output.fileno())
    if total != source.bytes or digest.hexdigest() != source.sha256:
        raise ModelStoreError("ONNX bundle file digest or size mismatch")


def _extract(archive: Path, destination: Path, source: ModelSource) -> None:
    expected = {item.archive_path: item for item in source.files}
    with tarfile.open(archive, "r:") as bundle:
        regular = {member.name: member for member in bundle if member.isfile()}
        unsafe = [member for member in bundle if not (member.isdir() or member.isfile())]
        if set(regular) != set(expected) or unsafe:
            raise ModelStoreError("model archive has an unexpected entry set")
        for archive_path, item in expected.items():
            member = regular[archive_path]
            if member.size != item.bytes:
                raise ModelStoreError(f"model archive size mismatch: {archive_path}")
            source_file = bundle.extractfile(member)
            if source_file is None:
                raise ModelStoreError(f"model archive entry is unreadable: {archive_path}")
            target = destination / item.filename
            with target.open("xb") as output:
                shutil.copyfileobj(source_file, output, length=1024 * 1024)


def fetch(store: Path) -> dict[str, Any]:
    source = load_registered_source()
    target = model_path(store, source)
    store.mkdir(mode=0o700, parents=True, exist_ok=True)
    if store.is_symlink() or not store.is_dir():
        raise ModelStoreError("model store must be a directory")
    if target.exists():
        verify_model(target, source)
        return _summary(source, target, reused=True)
    objects = target.parent
    objects.mkdir(mode=0o700, parents=True, exist_ok=True)
    if objects.is_symlink() or not objects.is_dir():
        raise ModelStoreError("model objects store must be a directory")
    temporary = Path(tempfile.mkdtemp(prefix=".staging-", dir=objects))
    archive = temporary / "model.tar"
    extracted = temporary / "model"
    extracted.mkdir(mode=0o700)
    try:
        _download(source, archive)
        _extract(archive, extracted, source)
        verify_model(extracted, source)
        try:
            extracted.rename(target)
        except OSError:
            if not target.exists():
                raise
            verify_model(target, source)
        verify_model(target, source)
        return _summary(source, target, reused=False)
    finally:
        shutil.rmtree(temporary, ignore_errors=True)


def fetch_onnx(store: Path) -> dict[str, Any]:
    source = load_registered_onnx_source()
    target = onnx_model_path(store, source)
    store.mkdir(mode=0o700, parents=True, exist_ok=True)
    if store.is_symlink() or not store.is_dir():
        raise ModelStoreError("model store must be a directory")
    objects = store / "objects"
    objects.mkdir(mode=0o700, parents=True, exist_ok=True)
    if objects.is_symlink() or not objects.is_dir():
        raise ModelStoreError("model objects store must be a directory")
    if target.exists():
        verify_onnx_model(target, source)
        return _onnx_summary(source, target, reused=True)
    temporary = Path(tempfile.mkdtemp(prefix=".staging-", dir=objects))
    staged = temporary / "inference.onnx"
    try:
        _download_onnx(source, staged)
        verify_onnx_model(staged, source)
        try:
            temporary.rename(target.parent)
        except OSError:
            if not target.exists():
                raise
            verify_onnx_model(target, source)
        verify_onnx_model(target, source)
        return _onnx_summary(source, target, reused=False)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary, ignore_errors=True)


def fetch_onnx_bundle(store: Path, model_id: str) -> dict[str, Any]:
    source = load_registered_onnx_bundle(model_id)
    target = onnx_bundle_path(store, source)
    _ensure_durable_directory(store)
    if store.is_symlink() or not store.is_dir():
        raise ModelStoreError("model store must be a directory")
    with _onnx_bundle_writer_lock(store):
        bundles = _ensure_onnx_bundle_store(store)
        _recover_onnx_bundle_staging(bundles)
        if target.exists():
            verify_onnx_bundle(target, source)
            return _onnx_bundle_summary(source, target, reused=True)
        count, total = _onnx_bundle_store_usage(bundles)
        incoming = sum(item.bytes for item in source.files)
        if (
            incoming > MAX_ONNX_BUNDLE_OBJECT_BYTES
            or count >= MAX_ONNX_BUNDLE_COUNT
            or total + incoming > MAX_ONNX_BUNDLE_BYTES
        ):
            raise ModelStoreError("ONNX bundle store is at capacity")
        temporary = Path(
            tempfile.mkdtemp(prefix=ONNX_BUNDLE_STAGING_PREFIX, dir=bundles)
        )
        marker = temporary / ONNX_BUNDLE_STAGING_MARKER
        try:
            with marker.open("xb") as output:
                output.write(b"scorepeek-owned-onnx-bundle-staging-v1\n")
                output.flush()
                os.fsync(output.fileno())
            _fsync_directory(temporary)
            _fsync_directory(bundles)
            for item in source.files:
                _download_onnx_bundle_file(item, temporary / item.filename)
            verify_onnx_bundle(temporary, source, staging=True)
            _fsync_directory(temporary)
            temporary.rename(target)
            _fsync_directory(bundles)
            (target / ONNX_BUNDLE_STAGING_MARKER).unlink()
            _fsync_directory(target)
            _fsync_directory(bundles)
            verify_onnx_bundle(target, source)
            return _onnx_bundle_summary(source, target, reused=False)
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)
                _fsync_directory(bundles)


def _summary(source: ModelSource, target: Path, *, reused: bool) -> dict[str, Any]:
    return {
        "schema": "scorepeek-ocr-model-fetch-summary-v1",
        "model_id": source.model_id,
        "model_name": source.model_name,
        "archive_sha256": source.archive_sha256,
        "model_dir": str(target),
        "reused": reused,
    }


def _onnx_summary(
    source: OnnxModelSource, target: Path, *, reused: bool
) -> dict[str, Any]:
    return {
        "schema": "scorepeek-ocr-onnx-model-fetch-summary-v1",
        "model_id": source.model_id,
        "model_name": source.model_name,
        "sha256": source.sha256,
        "model_file": str(target),
        "reused": reused,
    }


def _onnx_bundle_summary(
    source: OnnxBundleSource, target: Path, *, reused: bool
) -> dict[str, Any]:
    return {
        "schema": "scorepeek-ocr-onnx-model-bundle-fetch-summary-v1",
        "model_id": source.model_id,
        "model_name": source.model_name,
        "manifest_sha256": source.manifest_sha256,
        "bundle_dir": str(target),
        "reused": reused,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)
    fetch_parser = subcommands.add_parser("fetch")
    fetch_parser.add_argument("--store", type=Path, default=None)
    fetch_onnx_parser = subcommands.add_parser("fetch-onnx")
    fetch_onnx_parser.add_argument("--store", type=Path, default=None)
    fetch_onnx_bundle_parser = subcommands.add_parser("fetch-onnx-bundle")
    fetch_onnx_bundle_parser.add_argument("--model-id", required=True)
    fetch_onnx_bundle_parser.add_argument("--store", type=Path, default=None)
    arguments = parser.parse_args()
    try:
        store = arguments.store or default_store()
        if arguments.command == "fetch":
            result = fetch(store)
        elif arguments.command == "fetch-onnx":
            result = fetch_onnx(store)
        else:
            result = fetch_onnx_bundle(store, arguments.model_id)
    except (ModelStoreError, OSError, tarfile.TarError) as error:
        print(f"scorepeek OCR model fetch failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
