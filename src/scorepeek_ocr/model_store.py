"""Content-addressed acquisition for registered offline OCR models."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MAX_MANIFEST_BYTES = 64 * 1024
MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
PROJECT_ROOT = Path(__file__).resolve().parents[2]
REGISTERED_MODEL_MANIFEST = (
    PROJECT_ROOT / "models" / "manifests" / "pp-ocrv6-small-rec-v1.json"
)
REGISTERED_MODEL_MANIFEST_SHA256 = (
    "ccb361d69880cf98cb61a50bfbf9f6c5e46d76d6b0c93eed53ee9b99ec4d8ab8"
)


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


def _file_identity(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
    )


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


def _valid_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def default_store() -> Path:
    configured = os.environ.get("XDG_DATA_HOME")
    if configured:
        base = Path(configured)
    else:
        home = os.environ.get("HOME")
        if not home:
            raise ModelStoreError("HOME is required when XDG_DATA_HOME is unset")
        base = Path(home) / ".local" / "share"
    if not base.is_absolute():
        raise ModelStoreError("model store base must be absolute")
    return base / "scorepeek" / "models"


def model_path(store: Path, source: ModelSource) -> Path:
    if not store.is_absolute():
        raise ModelStoreError("model store must be absolute")
    return store / "objects" / source.archive_sha256


def verify_model(directory: Path, source: ModelSource) -> None:
    if directory.is_symlink() or not directory.is_dir():
        raise ModelStoreError("registered model object is not a directory")
    if {entry.name for entry in directory.iterdir()} != {
        item.filename for item in source.files
    }:
        raise ModelStoreError("registered model object has an unexpected file set")
    for item in source.files:
        digest, size = _sha256_file(directory / item.filename, item.bytes)
        if digest != item.sha256 or size != item.bytes:
            raise ModelStoreError(f"registered model file mismatch: {item.filename}")


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
    store.mkdir(mode=0o700, parents=True, exist_ok=True)
    if store.is_symlink() or not store.is_dir():
        raise ModelStoreError("model store must be a directory")
    target = model_path(store, source)
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


def _summary(source: ModelSource, target: Path, *, reused: bool) -> dict[str, Any]:
    return {
        "schema": "scorepeek-ocr-model-fetch-summary-v1",
        "model_id": source.model_id,
        "model_name": source.model_name,
        "archive_sha256": source.archive_sha256,
        "model_dir": str(target),
        "reused": reused,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)
    fetch_parser = subcommands.add_parser("fetch")
    fetch_parser.add_argument("--store", type=Path, default=None)
    arguments = parser.parse_args()
    try:
        store = arguments.store or default_store()
        result = fetch(store)
    except (ModelStoreError, OSError, tarfile.TarError) as error:
        print(f"scorepeek OCR model fetch failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
