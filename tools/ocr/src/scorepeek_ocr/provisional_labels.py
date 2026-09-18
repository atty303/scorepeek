"""Build fail-closed provisional title labels from reviewed music-list crops."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
import sys
import time
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
import unicodedata2 as unicodedata

from scorepeek_ocr.model_store import (
    ModelStoreError,
    default_store,
    load_registered_source,
    model_path,
    read_verified_model_files,
)
from scorepeek_ocr.spike import SpikeError, _write_output

SCHEMA = "scorepeek-private-provisional-music-list-title-labels-v1"
COMPARISON_KEY_ID = "scorepeek-title-nfc-ucd17-exact-then-ascii-width-fold-v2"
MINIMUM_CONFIDENCE = 0.95
MAX_INPUT_BYTES = 256 * 1024 * 1024
MAX_CANDIDATE_BYTES = 32 * 1024 * 1024
ALLOWED_PERMISSION_STATUS = {
    "permission_not_recorded",
    "private_development_only",
    "redistribution_permission_recorded",
}


class ProvisionalLabelError(Exception):
    """An input or OCR association violated the provisional-label contract."""


@dataclass(frozen=True)
class Variant:
    song_id: str
    value: str
    kind: str
    evidence_id: tuple[str, str, str]


@dataclass(frozen=True)
class CandidateIndex:
    exact: dict[str, list[Variant]]
    folded: dict[str, list[Variant]]


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _valid_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def _read_json(path: Path, expected_sha256: str, maximum: int) -> tuple[bytes, Any]:
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ProvisionalLabelError(f"input is not an absolute regular file: {path}")
    if not _valid_sha256(expected_sha256):
        raise ProvisionalLabelError("input SHA-256 is invalid")
    size = path.stat().st_size
    if not 0 < size <= maximum:
        raise ProvisionalLabelError(f"input size is outside the contract: {path}")
    data = path.read_bytes()
    if len(data) != size or _sha256(data) != expected_sha256:
        raise ProvisionalLabelError(f"input changed or digest mismatched: {path}")
    try:
        return data, json.loads(data)
    except json.JSONDecodeError as error:
        raise ProvisionalLabelError(f"input is invalid JSON: {path}") from error


def _exact_comparison_key(value: str) -> str:
    return "".join(
        character
        for character in unicodedata.normalize("NFC", value)
        if character != " "
    )


def _comparison_key(value: str) -> str:
    key = []
    for character in unicodedata.normalize("NFC", value):
        codepoint = ord(character)
        if character in {" ", "\u3000"}:
            continue
        if 0xFF01 <= codepoint <= 0xFF5E:
            character = chr(codepoint - 0xFEE0)
        key.append(character)
    return "".join(key)


def _load_candidates(raw: Any) -> tuple[str, dict[str, dict[str, Any]], CandidateIndex]:
    if (
        not isinstance(raw, dict)
        or set(raw)
        != {
            "schema",
            "catalog_sha256",
            "comparison_key_id",
            "domain",
            "source_evidence",
            "candidates",
        }
        or raw["schema"] != "scorepeek-private-provisional-title-candidates-v1"
        or not _valid_sha256(raw["catalog_sha256"])
        or raw["comparison_key_id"] != COMPARISON_KEY_ID
        or raw["domain"]
        != {
            "play_type": "single",
            "difficulty": "hyper",
            "infinitas_status": "confirmed_present",
        }
        or not isinstance(raw["source_evidence"], list)
        or not isinstance(raw["candidates"], list)
    ):
        raise ProvisionalLabelError("candidate artifact fields are invalid")
    evidence: dict[str, dict[str, Any]] = {}
    for item in raw["source_evidence"]:
        required = {
            "source_id",
            "lineage_id",
            "revision_strategy",
            "revision",
            "content_sha256",
            "byte_size",
            "record_count",
            "parser_version",
            "declared_scope",
            "completeness",
            "field_authority",
            "freshness",
            "rights_and_provenance",
        }
        if not isinstance(item, dict) or set(item) != required:
            raise ProvisionalLabelError("candidate source evidence fields are invalid")
        key = f'{item["source_id"]}:{item["revision"]}:{item["content_sha256"]}'
        if (
            key in evidence
            or not _valid_sha256(item["content_sha256"])
            or not isinstance(item["revision"], str)
            or not item["revision"]
            or not isinstance(item["rights_and_provenance"], str)
            or not item["rights_and_provenance"]
        ):
            raise ProvisionalLabelError("candidate source evidence values are invalid")
        evidence[key] = item

    exact: dict[str, list[Variant]] = defaultdict(list)
    folded: dict[str, list[Variant]] = defaultdict(list)
    seen_songs = set()
    for candidate in raw["candidates"]:
        if not isinstance(candidate, dict) or set(candidate) != {"song_id", "variants"}:
            raise ProvisionalLabelError("candidate song fields are invalid")
        song_id = candidate["song_id"]
        if not isinstance(song_id, str) or song_id in seen_songs:
            raise ProvisionalLabelError("candidate song ID is invalid or duplicated")
        seen_songs.add(song_id)
        if not isinstance(candidate["variants"], list) or not candidate["variants"]:
            raise ProvisionalLabelError("candidate variants are invalid")
        for item in candidate["variants"]:
            if not isinstance(item, dict) or set(item) != {
                "value",
                "source_id",
                "kind",
                "evidence_id",
            }:
                raise ProvisionalLabelError("candidate variant fields are invalid")
            evidence_id = item["evidence_id"]
            if not isinstance(evidence_id, dict) or set(evidence_id) != {
                "source_id",
                "revision",
                "content_sha256",
            }:
                raise ProvisionalLabelError("candidate variant evidence is invalid")
            evidence_key = (
                evidence_id["source_id"],
                evidence_id["revision"],
                evidence_id["content_sha256"],
            )
            if (
                item["kind"] == "search_term"
                or not isinstance(item["value"], str)
                or not item["value"]
                or f"{evidence_key[0]}:{evidence_key[1]}:{evidence_key[2]}" not in evidence
            ):
                raise ProvisionalLabelError("candidate variant values are invalid")
            variant = Variant(song_id, item["value"], item["kind"], evidence_key)
            exact[_exact_comparison_key(variant.value)].append(variant)
            folded[_comparison_key(variant.value)].append(variant)
    return raw["catalog_sha256"], evidence, CandidateIndex(exact, folded)


def _associate(
    observed_text: str,
    confidence: float,
    candidates: CandidateIndex,
) -> tuple[str, list[Variant]]:
    if not isinstance(observed_text, str) or not isinstance(confidence, float):
        raise ProvisionalLabelError("OCR output type is invalid")
    if not confidence >= MINIMUM_CONFIDENCE:
        return "low_confidence", []
    variants = candidates.exact.get(_exact_comparison_key(observed_text), [])
    if not variants:
        variants = candidates.folded.get(_comparison_key(observed_text), [])
    if not variants:
        return "no_exact_catalog_candidate", []
    song_ids = {variant.song_id for variant in variants}
    if len(song_ids) != 1:
        return "ambiguous_catalog_songs", []
    values = {variant.value for variant in variants}
    if len(values) != 1:
        return "ambiguous_display_text", []
    value = next(iter(values))
    return "unique", [variant for variant in variants if variant.value == value]


def _load_reviewed_groups(
    disposition: Any,
    plan: Any,
    source_artifact: Any,
    disposition_sha256: str,
    plan_sha256: str,
    source_artifact_sha256: str,
    expected_eligible_groups: int,
) -> tuple[str, list[dict[str, Any]]]:
    if (
        not isinstance(plan, dict)
        or plan.get("schema") != "scorepeek-private-music-list-motion-review-plan-v1"
        or plan.get("source_artifact_sha256") != source_artifact_sha256
        or not _valid_sha256(plan.get("catalog_sha256"))
        or not isinstance(plan.get("groups"), list)
    ):
        raise ProvisionalLabelError("review plan binding is invalid")
    if (
        not isinstance(disposition, dict)
        or disposition.get("schema")
        != "scorepeek-private-music-list-motion-review-disposition-v1"
        or disposition.get("source_review_plan_sha256") != plan_sha256
        or not isinstance(disposition.get("dispositions"), list)
        or len(disposition["dispositions"]) != len(plan["groups"])
    ):
        raise ProvisionalLabelError("review disposition binding is invalid")
    if (
        not isinstance(source_artifact, dict)
        or source_artifact.get("schema")
        != "scorepeek-private-music-list-motion-artifact-v1"
        or source_artifact.get("catalog_sha256") != plan["catalog_sha256"]
    ):
        raise ProvisionalLabelError("reviewed artifact binding is invalid")

    selected = []
    seen_pixels = set()
    for index, (decision, group) in enumerate(
        zip(disposition["dispositions"], plan["groups"], strict=True)
    ):
        if (
            not isinstance(decision, dict)
            or decision.get("group_id") != f"G{index:05}"
            or decision.get("crop_pixel_sha256") != group.get("crop_pixel_sha256")
        ):
            raise ProvisionalLabelError("review group ordering or digest is invalid")
        annotation = decision.get("annotation")
        decided = decision.get("status") == "decided"
        if decided and not isinstance(annotation, dict):
            raise ProvisionalLabelError("review group annotation is invalid")
        presentation = annotation.get("presentation") if decided else None
        occurrences = group.get("occurrences")
        occurrence_count = decision.get("occurrence_count")
        if (
            not isinstance(occurrences, list)
            or not occurrences
            or type(occurrence_count) is not int
            or occurrence_count <= 0
            or occurrence_count != len(occurrences)
        ):
            raise ProvisionalLabelError("review group occurrence count is invalid")
        occurrence_states = []
        for occurrence in occurrences:
            pair_motion = occurrence.get("pair_motion") if isinstance(occurrence, dict) else None
            state = pair_motion.get("state") if isinstance(pair_motion, dict) else None
            if state not in {"stationary", "scrolling", "unknown"}:
                raise ProvisionalLabelError("review group pair motion is invalid")
            occurrence_states.append(state)
        eligible = (
            decided
            and annotation.get("content") == "title"
            and isinstance(presentation, dict)
            and presentation.get("availability") == "available"
            and presentation.get("color_domain") == "standard"
            and all(state == "stationary" for state in occurrence_states)
        )
        if not eligible:
            continue
        pixel_sha256 = decision["crop_pixel_sha256"]
        if (
            pixel_sha256 in seen_pixels
            or not _valid_sha256(pixel_sha256)
        ):
            raise ProvisionalLabelError("eligible review group is invalid")
        occurrence = occurrences[0]
        if not isinstance(occurrence, dict) or not _valid_sha256(
            occurrence.get("crop_file_sha256")
        ):
            raise ProvisionalLabelError("eligible crop occurrence is invalid")
        path = Path(occurrence.get("crop_path", ""))
        if not path.is_absolute():
            raise ProvisionalLabelError("eligible crop path is not absolute")
        seen_pixels.add(pixel_sha256)
        selected.append(
            {
                "group_id": decision["group_id"],
                "crop_pixel_sha256": pixel_sha256,
                "crop_file_sha256": occurrence["crop_file_sha256"],
                "crop_path": path,
                "occurrence_count": decision["occurrence_count"],
            }
        )
    if expected_eligible_groups <= 0 or len(selected) != expected_eligible_groups:
        raise ProvisionalLabelError(
            "eligible review group count is "
            f"{len(selected)}, expected {expected_eligible_groups}"
        )
    return plan["catalog_sha256"], selected


def _read_verified_crop(group: dict[str, Any]) -> bytes:
    path = group["crop_path"]
    if path.is_symlink() or not path.is_file():
        raise ProvisionalLabelError(f"crop is not a regular file: {path}")
    size = path.stat().st_size
    if not 16 < size <= 4 * 1024 * 1024:
        raise ProvisionalLabelError(f"crop size is outside the contract: {path}")
    data = path.read_bytes()
    if len(data) != size or _sha256(data) != group["crop_file_sha256"]:
        raise ProvisionalLabelError(f"crop file digest mismatched: {path}")
    first, second, remainder = data.split(b"\n", 2)
    if first != b"P6" or second != b"475 45" or not remainder.startswith(b"255\n"):
        raise ProvisionalLabelError(f"crop PPM contract mismatched: {path}")
    pixels = remainder[4:]
    if len(pixels) != 475 * 45 * 3 or _sha256(pixels) != group["crop_pixel_sha256"]:
        raise ProvisionalLabelError(f"crop pixel digest mismatched: {path}")
    return data


def _predict(groups: list[dict[str, Any]], model_store: Path) -> tuple[Any, list[tuple[str, float]], float]:
    import cv2

    source = load_registered_source()
    directory = model_path(model_store, source)
    files = read_verified_model_files(directory, source)
    installed = {
        "paddleocr": importlib.metadata.version("paddleocr"),
        "paddlepaddle": importlib.metadata.version("paddlepaddle"),
    }
    if installed != {
        "paddleocr": source.paddleocr_version,
        "paddlepaddle": source.paddlepaddle_version,
    }:
        raise ProvisionalLabelError("installed OCR package versions do not match registration")
    os.environ["PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK"] = "True"
    from paddleocr import TextRecognition

    import tempfile

    started = time.perf_counter()
    results: list[tuple[str, float]] = []
    with tempfile.TemporaryDirectory(prefix="scorepeek-provisional-ocr-") as temporary:
        snapshot = Path(temporary) / "model"
        snapshot.mkdir()
        for filename, data in files.items():
            (snapshot / filename).write_bytes(data)
        predictor = TextRecognition(
            model_name=source.model_name,
            model_dir=str(snapshot),
            device="cpu",
            enable_hpi=False,
        )
        try:
            for offset in range(0, len(groups), 32):
                images = []
                for group in groups[offset : offset + 32]:
                    data = _read_verified_crop(group)
                    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
                    if image is None or image.shape != (45, 475, 3):
                        raise ProvisionalLabelError("verified crop could not be decoded")
                    images.append(image)
                outputs = predictor.predict(input=images, batch_size=len(images))
                if len(outputs) != len(images):
                    raise ProvisionalLabelError("OCR output count does not match crop count")
                for output in outputs:
                    text = output.get("rec_text")
                    score = output.get("rec_score")
                    if (
                        not isinstance(text, str)
                        or not isinstance(score, float)
                        or not 0 <= score <= 1
                    ):
                        raise ProvisionalLabelError("OCR output values are invalid")
                    results.append((text, score))
        finally:
            predictor.close()
    return source, results, round((time.perf_counter() - started) * 1000, 3)


def generate(
    disposition_path: Path,
    disposition_sha256: str,
    plan_path: Path,
    plan_sha256: str,
    source_artifact_path: Path,
    source_artifact_sha256: str,
    candidate_path: Path,
    candidate_sha256: str,
    permission_status: str,
    expected_eligible_groups: int,
    model_store: Path,
) -> dict[str, Any]:
    if permission_status not in ALLOWED_PERMISSION_STATUS:
        raise ProvisionalLabelError("permission status is invalid")
    _, disposition = _read_json(disposition_path, disposition_sha256, MAX_INPUT_BYTES)
    _, plan = _read_json(plan_path, plan_sha256, MAX_INPUT_BYTES)
    _, source_artifact = _read_json(
        source_artifact_path, source_artifact_sha256, MAX_INPUT_BYTES
    )
    _, candidate_raw = _read_json(candidate_path, candidate_sha256, MAX_CANDIDATE_BYTES)
    review_catalog, groups = _load_reviewed_groups(
        disposition,
        plan,
        source_artifact,
        disposition_sha256,
        plan_sha256,
        source_artifact_sha256,
        expected_eligible_groups,
    )
    candidate_catalog, evidence, candidates = _load_candidates(candidate_raw)
    if review_catalog != candidate_catalog:
        raise ProvisionalLabelError("review and candidate catalog digests differ")
    source, outputs, elapsed_ms = _predict(groups, model_store)

    labels = []
    unknowns = []
    reasons: Counter[str] = Counter()
    for group, (text, score) in zip(groups, outputs, strict=True):
        state, variants = _associate(text, score, candidates)
        common = {
            "group_id": group["group_id"],
            "crop_pixel_sha256": group["crop_pixel_sha256"],
            "crop_file_sha256": group["crop_file_sha256"],
            "occurrence_count": group["occurrence_count"],
            "observed_text": text,
            "ocr_confidence": score,
        }
        if state != "unique":
            reasons[state] += 1
            unknowns.append({**common, "reason": state})
            continue
        provenance = []
        seen = set()
        for variant in variants:
            key = f"{variant.evidence_id[0]}:{variant.evidence_id[1]}:{variant.evidence_id[2]}"
            item = evidence[key]
            source_record = {
                "source_id": item["source_id"],
                "lineage_id": item["lineage_id"],
                "revision": item["revision"],
                "content_sha256": item["content_sha256"],
                "rights_and_provenance": item["rights_and_provenance"],
            }
            encoded = json.dumps(source_record, sort_keys=True)
            if encoded not in seen:
                seen.add(encoded)
                provenance.append(source_record)
        labels.append(
            {
                **common,
                "song_id": variants[0].song_id,
                "title": variants[0].value,
                "variant_kinds": sorted({variant.kind for variant in variants}),
                "source_provenance": provenance,
                "permission_status": permission_status,
            }
        )
    return {
        "schema": SCHEMA,
        "catalog_sha256": review_catalog,
        "review_disposition_sha256": disposition_sha256,
        "review_plan_sha256": plan_sha256,
        "source_artifact_sha256": source_artifact_sha256,
        "candidate_artifact_sha256": candidate_sha256,
        "comparison_key_id": COMPARISON_KEY_ID,
        "candidate_domain": candidate_raw["domain"],
        "minimum_confidence": MINIMUM_CONFIDENCE,
        "model_id": source.model_id,
        "model_archive_sha256": source.archive_sha256,
        "permission_status": permission_status,
        "elapsed_ms": elapsed_ms,
        "expected_eligible_group_count": expected_eligible_groups,
        "eligible_group_count": len(groups),
        "provisional_label_count": len(labels),
        "unknown_count": len(unknowns),
        "unknown_reason_counts": dict(sorted(reasons.items())),
        "labels": labels,
        "unknowns": unknowns,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--review-disposition", type=Path, required=True)
    parser.add_argument("--review-disposition-sha256", required=True)
    parser.add_argument("--review-plan", type=Path, required=True)
    parser.add_argument("--review-plan-sha256", required=True)
    parser.add_argument("--source-artifact", type=Path, required=True)
    parser.add_argument("--source-artifact-sha256", required=True)
    parser.add_argument("--candidates", type=Path, required=True)
    parser.add_argument("--candidates-sha256", required=True)
    parser.add_argument("--permission-status", choices=sorted(ALLOWED_PERMISSION_STATUS), required=True)
    parser.add_argument("--expected-eligible-groups", type=int, required=True)
    parser.add_argument("--model-store", type=Path, default=None)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = generate(
            arguments.review_disposition,
            arguments.review_disposition_sha256,
            arguments.review_plan,
            arguments.review_plan_sha256,
            arguments.source_artifact,
            arguments.source_artifact_sha256,
            arguments.candidates,
            arguments.candidates_sha256,
            arguments.permission_status,
            arguments.expected_eligible_groups,
            arguments.model_store or default_store(),
        )
        encoded = json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n"
        _write_output(arguments.output, encoded)
    except (ProvisionalLabelError, SpikeError, ModelStoreError, OSError) as error:
        print(f"scorepeek provisional labeling failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    summary = {key: result[key] for key in (
        "schema",
        "catalog_sha256",
        "eligible_group_count",
        "provisional_label_count",
        "unknown_count",
        "unknown_reason_counts",
        "elapsed_ms",
    )}
    summary["output"] = str(arguments.output)
    summary["artifact_sha256"] = _sha256(encoded.encode())
    print(json.dumps(summary, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
