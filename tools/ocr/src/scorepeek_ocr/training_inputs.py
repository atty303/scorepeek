"""Seal reviewed music-list labels into a title-disjoint private training manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import uuid
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from scorepeek_ocr.provisional_labels import _valid_sha256
from scorepeek_ocr.spike import SpikeError, _write_output

SCHEMA = "scorepeek-private-title-training-input-manifest-v1"
FINAL_LABEL_SCHEMA = "scorepeek-private-final-music-list-title-labels-v1"
MAX_INPUT_BYTES = 256 * 1024 * 1024
SPLIT_CONTRACT_ID = "scorepeek-title-song-disjoint-sha256-80-10-10-v1"
PERMISSION_STATUS = "permission_not_recorded"


class TrainingInputError(Exception):
    """A private training-input artifact violated its immutable contract."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read(path: Path, digest: str) -> Any:
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise TrainingInputError("input is not an absolute regular file")
    if not _valid_sha256(digest):
        raise TrainingInputError("input SHA-256 is invalid")
    size = path.stat().st_size
    if not 0 < size <= MAX_INPUT_BYTES:
        raise TrainingInputError("input size is outside the contract")
    data = path.read_bytes()
    if len(data) != size or _sha256(data) != digest:
        raise TrainingInputError("input changed or digest mismatched")
    try:
        return json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingInputError("input is invalid JSON") from error


def _split(song_id: str) -> str:
    bucket = int.from_bytes(
        hashlib.sha256(f"{SPLIT_CONTRACT_ID}\0{song_id}".encode()).digest()[:8], "big"
    ) % 100
    return "train" if bucket < 80 else "validation" if bucket < 90 else "evaluation"


def _require_digest(value: Any, name: str) -> str:
    if not _valid_sha256(value):
        raise TrainingInputError(f"{name} is invalid")
    return value


def _load_final_labels(
    raw: Any,
    *,
    candidate_sha256: str,
    automated_label_sha256: str,
    visual_audit_sha256: str,
    source_artifact_sha256: str,
    crop_artifact_sha256: str,
) -> list[dict[str, Any]]:
    required = {
        "schema", "candidate_artifact_sha256", "automated_label_sha256",
        "visual_audit_sha256", "source_artifact_sha256", "crop_artifact_sha256", "labels",
    }
    if not isinstance(raw, dict) or set(raw) != required or raw["schema"] != FINAL_LABEL_SCHEMA:
        raise TrainingInputError("final label artifact fields are invalid")
    bindings = {
        "candidate_artifact_sha256": candidate_sha256,
        "automated_label_sha256": automated_label_sha256,
        "visual_audit_sha256": visual_audit_sha256,
        "source_artifact_sha256": source_artifact_sha256,
        "crop_artifact_sha256": crop_artifact_sha256,
    }
    if any(raw[key] != value for key, value in bindings.items()) or not isinstance(raw["labels"], list):
        raise TrainingInputError("final label artifact bindings are invalid")
    labels = []
    seen_groups = set()
    for label in raw["labels"]:
        required_label = {
            "group_id", "crop_pixel_sha256", "crop_file_sha256", "occurrence_count",
            "song_id", "title", "origin", "permission_status",
        }
        if not isinstance(label, dict) or set(label) != required_label:
            raise TrainingInputError("final label fields are invalid")
        try:
            parsed_song_id = uuid.UUID(label["song_id"])
        except (AttributeError, ValueError) as error:
            raise TrainingInputError("final label song ID is invalid") from error
        if (
            str(parsed_song_id) != label["song_id"]
            or not isinstance(label["group_id"], str)
            or not label["group_id"]
            or label["group_id"] in seen_groups
            or not _valid_sha256(label["crop_pixel_sha256"])
            or not _valid_sha256(label["crop_file_sha256"])
            or type(label["occurrence_count"]) is not int
            or label["occurrence_count"] <= 0
            or not isinstance(label["title"], str)
            or not label["title"]
            or label["origin"] != "music_list"
            or label["permission_status"] != PERMISSION_STATUS
        ):
            raise TrainingInputError("final label values are invalid")
        seen_groups.add(label["group_id"])
        labels.append(label)
    if not labels:
        raise TrainingInputError("final labels are empty")
    return labels


def generate(
    candidate: Path, candidate_sha256: str, automated_labels: Path, automated_label_sha256: str,
    visual_audit: Path, visual_audit_sha256: str, final_labels: Path, final_label_sha256: str,
    source_artifact: Path, source_artifact_sha256: str, crop_artifact: Path, crop_artifact_sha256: str,
) -> dict[str, Any]:
    candidate_raw = _read(candidate, candidate_sha256)
    automated_raw = _read(automated_labels, automated_label_sha256)
    audit_raw = _read(visual_audit, visual_audit_sha256)
    final_raw = _read(final_labels, final_label_sha256)
    source_raw = _read(source_artifact, source_artifact_sha256)
    crop_raw = _read(crop_artifact, crop_artifact_sha256)
    if (
        not isinstance(candidate_raw, dict)
        or candidate_raw.get("schema") != "scorepeek-private-provisional-title-candidates-v1"
        or not isinstance(automated_raw, dict)
        or automated_raw.get("schema") != "scorepeek-private-provisional-music-list-title-labels-v1"
        or not isinstance(audit_raw, dict)
        or not isinstance(source_raw, dict)
        or not isinstance(crop_raw, dict)
    ):
        raise TrainingInputError("bound source artifact schema is invalid")
    labels = _load_final_labels(
        final_raw, candidate_sha256=candidate_sha256, automated_label_sha256=automated_label_sha256,
        visual_audit_sha256=visual_audit_sha256, source_artifact_sha256=source_artifact_sha256,
        crop_artifact_sha256=crop_artifact_sha256,
    )
    song_splits = {label["song_id"]: _split(label["song_id"]) for label in labels}
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for label in labels:
        grouped[song_splits[label["song_id"]]].append({
            "group_id": label["group_id"], "song_id": label["song_id"], "title": label["title"],
            "crop_pixel_sha256": label["crop_pixel_sha256"], "crop_file_sha256": label["crop_file_sha256"],
            "occurrence_count": label["occurrence_count"], "origin": "music_list",
            "permission_status": PERMISSION_STATUS,
        })
    for entries in grouped.values():
        entries.sort(key=lambda value: value["group_id"])
    counts = Counter(song_splits.values())
    return {
        "schema": SCHEMA,
        "split_contract_id": SPLIT_CONTRACT_ID,
        "candidate_artifact_sha256": candidate_sha256,
        "automated_label_sha256": automated_label_sha256,
        "visual_audit_sha256": visual_audit_sha256,
        "final_label_sha256": final_label_sha256,
        "source_artifact_sha256": source_artifact_sha256,
        "crop_artifact_sha256": crop_artifact_sha256,
        "origin": "music_list",
        "permission_status": PERMISSION_STATUS,
        "provisional": True,
        "accepted_holdout_truth": False,
        "song_count": len(song_splits),
        "label_count": len(labels),
        "split_song_counts": {split: counts[split] for split in ("train", "validation", "evaluation")},
        "splits": {split: grouped[split] for split in ("train", "validation", "evaluation")},
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    for name in ("candidate", "automated-labels", "visual-audit", "final-labels", "source-artifact", "crop-artifact"):
        parser.add_argument(f"--{name}", type=Path, required=True)
        parser.add_argument(f"--{name}-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = generate(
            arguments.candidate, arguments.candidate_sha256, arguments.automated_labels, arguments.automated_labels_sha256,
            arguments.visual_audit, arguments.visual_audit_sha256, arguments.final_labels, arguments.final_labels_sha256,
            arguments.source_artifact, arguments.source_artifact_sha256, arguments.crop_artifact, arguments.crop_artifact_sha256,
        )
        encoded = json.dumps(result, ensure_ascii=False, separators=(",", ":"), allow_nan=False) + "\n"
        _write_output(arguments.output, encoded)
    except (TrainingInputError, SpikeError, OSError) as error:
        print(f"scorepeek training-input manifest failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps({"schema": SCHEMA, "output": str(arguments.output), "artifact_sha256": _sha256(encoded.encode())}, separators=(",", ":")))


if __name__ == "__main__":
    main()
