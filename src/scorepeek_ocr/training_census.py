"""Measure unique catalog-song coverage across a complete private title corpus."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import shutil
import sys
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any

import paddle
import yaml

from scorepeek_ocr.provisional_labels import _load_candidates
from scorepeek_ocr.training_artifacts import (
    MAX_MODEL_FILE_BYTES,
    _prepared_manifest,
    _training_labels,
    prepared_rows,
)
from scorepeek_ocr.training_catalog import (
    CatalogDecisions,
    CatalogTrie,
    evaluate_catalog,
    training_truth,
)
from scorepeek_ocr.training_initializer import (
    MAX_MANIFEST_BYTES,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_inputs import MAX_INPUT_BYTES
from scorepeek_ocr.training_pilot import _model
from scorepeek_ocr.training_source import load_registered_source, verify_source
from scorepeek_ocr.title_presentation import TRANSFORM_IDS

CENSUS_SCHEMA = "scorepeek-private-title-model-coverage-census-v2"
MODEL_ID = re.compile(r"[a-z0-9][a-z0-9._-]{0,63}")
SPLITS = ("train", "validation", "evaluation")


class TrainingCensusError(Exception):
    """The private title-model coverage census could not be completed."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _slice(decisions: CatalogDecisions, start: int, stop: int) -> CatalogDecisions:
    return CatalogDecisions(
        decisions.correct[start:stop],
        decisions.margins[start:stop],
        decisions.expected_song_ids[start:stop],
        decisions.predicted_song_ids[start:stop],
    )


def summarize_songs(
    decisions: CatalogDecisions,
    labels: list[dict[str, Any]],
) -> tuple[dict[str, int | float | None], list[dict[str, Any]]]:
    if not (
        len(decisions.correct)
        == len(decisions.margins)
        == len(decisions.expected_song_ids)
        == len(decisions.predicted_song_ids)
        == len(labels)
    ):
        raise TrainingCensusError("catalog decisions and labels differ")
    grouped: dict[str, list[int]] = defaultdict(list)
    for index, song_id in enumerate(decisions.expected_song_ids):
        if labels[index]["song_id"] != song_id:
            raise TrainingCensusError("catalog decisions and label truth differ")
        grouped[song_id].append(index)

    unrecognized = []
    fully_correct = 0
    for song_id in sorted(grouped):
        indexes = grouped[song_id]
        failures = []
        for index in indexes:
            if decisions.correct[index]:
                continue
            label = labels[index]
            failures.append(
                {
                    "group_id": label["group_id"],
                    "crop_file_sha256": label["crop_file_sha256"],
                    "crop_pixel_sha256": label["crop_pixel_sha256"],
                    "title": label["title"],
                    "predicted_song_id": decisions.predicted_song_ids[index],
                    "runner_up_margin": decisions.margins[index],
                }
            )
        if not failures:
            fully_correct += 1
            continue
        unrecognized.append(
            {
                "song_id": song_id,
                "title": labels[indexes[0]]["title"],
                "crop_count": len(indexes),
                "correct_crop_count": len(indexes) - len(failures),
                "failures": failures,
            }
        )

    correct_margins = [
        margin
        for correct, margin in zip(decisions.correct, decisions.margins, strict=True)
        if correct
    ]
    incorrect_margins = [
        margin
        for correct, margin in zip(decisions.correct, decisions.margins, strict=True)
        if not correct
    ]
    summary: dict[str, int | float | None] = {
        "sample_count": len(labels),
        "song_count": len(grouped),
        "fully_correct_song_count": fully_correct,
        "unrecognized_song_count": len(grouped) - fully_correct,
        "correct_unique_song_id_decision_count": sum(decisions.correct),
        "incorrect_or_tied_song_id_decision_count": len(labels) - sum(decisions.correct),
        "minimum_correct_runner_up_margin": min(correct_margins, default=None),
        "maximum_incorrect_runner_up_margin": max(incorrect_margins, default=None),
    }
    return summary, unrecognized


def run(
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    training_input: Path,
    training_input_sha256: str,
    catalog_candidates: Path,
    catalog_candidates_sha256: str,
    models: list[tuple[str, Path, str, str]],
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared_manifest(
        json.loads(
            _read_regular(
                preparation / "manifest.json",
                MAX_MANIFEST_BYTES,
                preparation_sha256,
            )
        )
    )
    if prepared["training_input_sha256"] != training_input_sha256:
        raise TrainingCensusError("training input is not bound to the preparation")
    try:
        training_raw = json.loads(
            _read_regular(training_input, MAX_INPUT_BYTES, training_input_sha256)
        )
        candidate_raw = json.loads(
            _read_regular(
                catalog_candidates,
                MAX_INPUT_BYTES,
                catalog_candidates_sha256,
            )
        )
    except json.JSONDecodeError as error:
        raise TrainingCensusError("coverage input is invalid JSON") from error
    labels_by_split = _training_labels(training_raw, training_input_sha256)
    candidate_catalog, _, _ = _load_candidates(candidate_raw)
    if candidate_catalog != prepared["catalog_sha256"]:
        raise TrainingCensusError("candidate catalog differs from the preparation")
    if not models or len(models) > 16:
        raise TrainingCensusError("model count is outside the census contract")
    identifiers = [model[0] for model in models]
    if len(set(identifiers)) != len(identifiers) or any(
        MODEL_ID.fullmatch(identifier) is None for identifier in identifiers
    ):
        raise TrainingCensusError("model IDs are invalid or duplicated")
    if any(transform not in TRANSFORM_IDS for _, _, _, transform in models):
        raise TrainingCensusError("model presentation transform is invalid")
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise TrainingCensusError("output must be a new absolute directory")

    config = yaml.safe_load(
        _read_regular(
            preparation / "training-config.yml",
            MAX_MANIFEST_BYTES,
            prepared["derived_training_config_sha256"],
        )
    )
    tokens = _read_regular(
        preparation / "dictionary.txt",
        MAX_MANIFEST_BYTES * 2,
        prepared["dictionary_sha256"],
    ).decode().splitlines()
    rows_by_split = {
        split: prepared_rows(preparation, prepared, split) for split in SPLITS
    }
    truth_by_split = {
        split: training_truth(rows_by_split[split], labels_by_split[split])
        for split in SPLITS
    }
    all_rows = [row for split in SPLITS for row in rows_by_split[split]]
    all_labels = [label for split in SPLITS for label in labels_by_split[split]]
    all_truth = [song_id for split in SPLITS for song_id in truth_by_split[split]]

    sys.path.insert(0, str(source_root))
    trie = CatalogTrie(
        candidate_raw["candidates"],
        ["blank", *tokens, " "],
        prepared["output_timesteps"],
        candidate_raw["comparison_key_id"],
    )
    records = []
    for identifier, checkpoint, checkpoint_sha256, transform in models:
        checkpoint_bytes = _read_regular(
            checkpoint, MAX_MODEL_FILE_BYTES, checkpoint_sha256
        )
        model, ctc_tokens = _model(config, tokens)
        model.set_state_dict(paddle.load(io.BytesIO(checkpoint_bytes)))
        probe, decisions = evaluate_catalog(
            model,
            all_rows,
            all_truth,
            ctc_tokens,
            prepared["model_input_width"],
            trie,
            transform,
        )
        overall, unrecognized = summarize_songs(decisions, all_labels)
        split_records = {}
        offset = 0
        for split in SPLITS:
            stop = offset + len(rows_by_split[split])
            split_summary, split_unrecognized = summarize_songs(
                _slice(decisions, offset, stop), labels_by_split[split]
            )
            split_records[split] = {
                "summary": split_summary,
                "unrecognized_song_ids": [row["song_id"] for row in split_unrecognized],
            }
            offset = stop
        records.append(
            {
                "model_id": identifier,
                "checkpoint_sha256": checkpoint_sha256,
                "presentation_transform_id": transform,
                "elapsed_ms": probe["elapsed_ms"],
                "overall": overall,
                "splits": split_records,
                "unrecognized_songs": unrecognized,
            }
        )
        print(
            "scorepeek training census progress: "
            f"{identifier} {overall['fully_correct_song_count']}/{overall['song_count']} "
            f"songs in {probe['elapsed_ms']} ms",
            file=sys.stderr,
            flush=True,
        )

    record = {
        "schema": CENSUS_SCHEMA,
        "training_preparation_sha256": preparation_sha256,
        "training_input_sha256": training_input_sha256,
        "catalog_candidate_artifact_sha256": catalog_candidates_sha256,
        "comparison_key_id": candidate_raw["comparison_key_id"],
        "catalog_sha256": prepared["catalog_sha256"],
        "provisional": True,
        "accepted_holdout_truth": False,
        "models": records,
    }
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        encoded = (
            json.dumps(record, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
            + "\n"
        ).encode()
        (staging / "manifest.json").write_bytes(encoded)
        _publish(staging, output)
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise
    return {**record, "artifact_sha256": _sha256(encoded)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preparation", type=Path, required=True)
    parser.add_argument("--preparation-sha256", required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--training-input", type=Path, required=True)
    parser.add_argument("--training-input-sha256", required=True)
    parser.add_argument("--catalog-candidates", type=Path, required=True)
    parser.add_argument("--catalog-candidates-sha256", required=True)
    parser.add_argument(
        "--model",
        action="append",
        nargs=4,
        metavar=("ID", "CHECKPOINT", "SHA256", "TRANSFORM"),
        required=True,
    )
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.source_root,
            arguments.training_input,
            arguments.training_input_sha256,
            arguments.catalog_candidates,
            arguments.catalog_candidates_sha256,
            [
                (identifier, Path(path), digest, transform)
                for identifier, path, digest, transform in arguments.model
            ],
            arguments.output,
        )
    except Exception as error:
        print(f"scorepeek training census failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(
        json.dumps(
            {
                "schema": CENSUS_SCHEMA,
                "output": str(arguments.output),
                "artifact_sha256": result["artifact_sha256"],
                "models": [
                    {
                        "model_id": model["model_id"],
                        **model["overall"],
                    }
                    for model in result["models"]
                ],
            },
            separators=(",", ":"),
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
