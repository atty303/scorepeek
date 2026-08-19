"""Verify the registered PaddleOCR checkout before offline training or export."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from scorepeek_ocr.model_store import MAX_MANIFEST_BYTES, _read_regular_bytes, _valid_sha256

PROJECT_ROOT = Path(__file__).resolve().parents[2]
REGISTERED_TRAINING_SOURCE_MANIFEST = (
    PROJECT_ROOT / "models" / "manifests" / "paddleocr-v3.7.0-training-source.json"
)
REGISTERED_TRAINING_SOURCE_MANIFEST_SHA256 = (
    "5fd44ea5bda24763b5d90a38486dbcddb3151780df473cb412da122acb091a93"
)
MAX_SOURCE_FILE_BYTES = 16 * 1024 * 1024


class TrainingSourceError(Exception):
    """The selected PaddleOCR checkout does not match the registered source."""


@dataclass(frozen=True)
class SourceFile:
    path: str
    sha256: str


@dataclass(frozen=True)
class TrainingSource:
    source_url: str
    commit: str
    license_id: str
    training_entrypoint: SourceFile
    export_entrypoint: SourceFile
    small_rec_config: SourceFile
    requirements: SourceFile


def _exact_object(value: Any, keys: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise TrainingSourceError(f"invalid {context} fields")
    return value


def _load_file(value: Any, context: str) -> SourceFile:
    raw = _exact_object(value, {"path", "sha256"}, context)
    path = raw["path"]
    if (
        not isinstance(path, str)
        or not path
        or Path(path).is_absolute()
        or ".." in Path(path).parts
        or not _valid_sha256(raw["sha256"])
    ):
        raise TrainingSourceError(f"invalid {context} values")
    return SourceFile(path=path, sha256=raw["sha256"])


def _load_source(data: bytes) -> TrainingSource:
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingSourceError("training source manifest is invalid") from error
    raw = _exact_object(
        raw,
        {
            "schema", "source_url", "commit", "license_id", "training_entrypoint",
            "export_entrypoint", "small_rec_config", "requirements",
        },
        "training source manifest",
    )
    if (
        raw["schema"] != "scorepeek-ocr-training-source-v1"
        or raw["source_url"] != "https://github.com/PaddlePaddle/PaddleOCR.git"
        or raw["commit"] != "b03f46425e8ff4442b268ce449e3eef758146cd4"
        or raw["license_id"] != "Apache-2.0"
    ):
        raise TrainingSourceError("training source manifest values are invalid")
    source = TrainingSource(
        source_url=raw["source_url"],
        commit=raw["commit"],
        license_id=raw["license_id"],
        training_entrypoint=_load_file(raw["training_entrypoint"], "training entrypoint"),
        export_entrypoint=_load_file(raw["export_entrypoint"], "export entrypoint"),
        small_rec_config=_load_file(raw["small_rec_config"], "small recognition config"),
        requirements=_load_file(raw["requirements"], "requirements"),
    )
    if {item.path for item in source_files(source)} != {
        "tools/train.py", "tools/export_model.py",
        "configs/rec/PP-OCRv6/PP-OCRv6_small_rec.yml", "requirements.txt",
    }:
        raise TrainingSourceError("training source file set is invalid")
    return source


def load_registered_source() -> TrainingSource:
    data = _read_regular_bytes(REGISTERED_TRAINING_SOURCE_MANIFEST, MAX_MANIFEST_BYTES)
    if hashlib.sha256(data).hexdigest() != REGISTERED_TRAINING_SOURCE_MANIFEST_SHA256:
        raise TrainingSourceError("registered training source manifest digest mismatch")
    return _load_source(data)


def source_files(source: TrainingSource) -> tuple[SourceFile, ...]:
    return (
        source.training_entrypoint,
        source.export_entrypoint,
        source.small_rec_config,
        source.requirements,
    )


def _sha256_regular_file(path: Path) -> str:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_SOURCE_FILE_BYTES:
            raise TrainingSourceError(f"source file is not within bounds: {path}")
        digest = hashlib.sha256()
        while chunk := os.read(descriptor, 1024 * 1024):
            digest.update(chunk)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
        after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns,
    ):
        raise TrainingSourceError(f"source file changed while reading: {path}")
    return digest.hexdigest()


def _head(root: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--verify", "HEAD^{commit}"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise TrainingSourceError("source checkout has no resolved commit")
    return completed.stdout.strip()


def verify_source(root: Path, source: TrainingSource) -> None:
    if not root.is_absolute() or root.is_symlink() or not root.is_dir():
        raise TrainingSourceError("source checkout is not an absolute directory")
    if _head(root) != source.commit:
        raise TrainingSourceError("source checkout commit mismatch")
    for item in source_files(source):
        if _sha256_regular_file(root / item.path) != item.sha256:
            raise TrainingSourceError(f"source file digest mismatch: {item.path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        source = load_registered_source()
        verify_source(arguments.source, source)
    except (OSError, TrainingSourceError) as error:
        print(f"scorepeek training source verification failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps({"schema": "scorepeek-ocr-training-source-v1", "source": str(arguments.source), "commit": source.commit}, separators=(",", ":")))


if __name__ == "__main__":
    main()
