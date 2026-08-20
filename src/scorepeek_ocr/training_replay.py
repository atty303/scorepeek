"""Compare the mapped initializer and selected pilot on private replay inputs."""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import numpy as np
import paddle
import yaml

from scorepeek_ocr.provisional_labels import _exact_comparison_key, _load_candidates
from scorepeek_ocr.spike import load_crops
from scorepeek_ocr.training_artifacts import (
    MAX_MODEL_FILE_BYTES,
    _prepared_manifest,
    _training_labels,
    _verify_prepared_files,
    prepared_rows,
)
from scorepeek_ocr.training_catalog import CatalogTrie, evaluate_catalog, training_truth
from scorepeek_ocr.training_initializer import (
    MAX_MANIFEST_BYTES,
    _decode,
    _preprocess,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_pilot import _initializer, _model
from scorepeek_ocr.training_inputs import MAX_INPUT_BYTES
from scorepeek_ocr.training_source import load_registered_source, verify_source
from scorepeek_ocr.title_presentation import IDENTITY_TRANSFORM_ID, TRANSFORM_IDS

REQUEST_SCHEMA = "scorepeek-private-title-model-result-replay-request-v1"
REPLAY_SCHEMA = "scorepeek-private-title-model-replay-v2"


class TrainingReplayError(Exception):
    """The private title-model replay inputs or results are invalid."""


def _pilot(path: Path, expected_sha256: str, preparation_sha256: str) -> dict[str, Any]:
    data = _read_regular(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    try:
        record = json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingReplayError("pilot manifest is invalid JSON") from error
    checkpoint = record.get("selected_checkpoint") if isinstance(record, dict) else None
    recipe = record.get("recipe") if isinstance(record, dict) else None
    if (
        record.get("schema") == "scorepeek-private-title-model-training-pilot-v1"
        and isinstance(recipe, dict)
    ):
        record = dict(record)
        recipe = dict(recipe)
        recipe.setdefault("presentation_transform_id", IDENTITY_TRANSFORM_ID)
        record["recipe"] = recipe
    if (
        record.get("schema")
        not in {
            "scorepeek-private-title-model-training-pilot-v1",
            "scorepeek-private-title-model-training-pilot-v2",
        }
        or record.get("training_preparation_sha256") != preparation_sha256
        or record.get("selected_steps") not in (1, 2, 4)
        or record.get("provisional") is not True
        or record.get("accepted_holdout_truth") is not False
        or record.get("permission_status") != "permission_not_recorded"
        or not isinstance(recipe, dict)
        or recipe.get("presentation_transform_id") not in TRANSFORM_IDS
        or not isinstance(checkpoint, dict)
        or set(checkpoint) != {"sha256", "bytes"}
    ):
        raise TrainingReplayError("pilot manifest values are invalid")
    checkpoint_data = _read_regular(
        path / "model.pdparams", MAX_MODEL_FILE_BYTES, checkpoint["sha256"]
    )
    if len(checkpoint_data) != checkpoint["bytes"]:
        raise TrainingReplayError("pilot checkpoint size mismatched")
    return record


def _result_rows(
    request_path: Path, request_sha256: str
) -> tuple[list[tuple[str, str, str]], dict[str, Any], list[dict[str, Any]]]:
    request_data = _read_regular(request_path, MAX_MANIFEST_BYTES, request_sha256)
    try:
        request = json.loads(request_data)
    except json.JSONDecodeError as error:
        raise TrainingReplayError("result replay request is invalid JSON") from error
    observations = request.get("observations") if isinstance(request, dict) else None
    if request.get("schema") != REQUEST_SCHEMA or not isinstance(observations, list) or not observations:
        raise TrainingReplayError("result replay request values are invalid")
    rows: list[tuple[str, str, str]] = []
    provenance: list[dict[str, Any]] = []
    for observation in observations:
        if not isinstance(observation, dict) or set(observation) != {
            "crop_directory",
            "crop_manifest_sha256",
            "expected_title",
            "source_pts",
        }:
            raise TrainingReplayError("result replay observation is invalid")
        directory = Path(observation["crop_directory"])
        if not directory.is_absolute() or not isinstance(observation["source_pts"], int):
            raise TrainingReplayError("result replay observation path or PTS is invalid")
        frame_extraction_sha256, crops = load_crops(
            directory, observation["crop_manifest_sha256"]
        )
        titles = [crop for crop in crops if crop.field == "title"]
        if len(titles) != 1:
            raise TrainingReplayError("result crop manifest has no unique title")
        title = titles[0]
        expected = observation["expected_title"]
        if not isinstance(expected, str) or not expected:
            raise TrainingReplayError("result expected title is invalid")
        rows.append((str(title.path), expected, title.file_sha256))
        manifest = json.loads(
            _read_regular(
                directory / "manifest.json",
                MAX_MANIFEST_BYTES,
                observation["crop_manifest_sha256"],
            )
        )
        provenance.append(
            {
                "crop_manifest_sha256": observation["crop_manifest_sha256"],
                "frame_extraction_sha256": frame_extraction_sha256,
                "canonical_frame_sha256": manifest["canonical_frame_sha256"],
                "normalizer_artifact_sha256": manifest["normalizer_artifact_sha256"],
                "canonical_layout_sha256": manifest["canonical_layout_sha256"],
                "title_crop_file_sha256": title.file_sha256,
            }
        )
    return rows, request, provenance


def _infer(
    model,
    rows: list[tuple[str, str, str]],
    tokens: list[str],
    width: int,
    presentation_transform_id: str,
) -> dict[str, Any]:
    started = time.perf_counter()
    predictions: list[str] = []
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
            tensors = model(paddle.to_tensor(images)).numpy()
            predictions.extend(_decode(tensor, tokens) for tensor in tensors)
    open_text_exact = [
        prediction == expected
        for prediction, (_, expected, _) in zip(predictions, rows, strict=True)
    ]
    comparison_key_exact = [
        _exact_comparison_key(prediction) == _exact_comparison_key(expected)
        for prediction, (_, expected, _) in zip(predictions, rows, strict=True)
    ]
    return {
        "sample_count": len(rows),
        "open_text_exact_count": sum(open_text_exact),
        "comparison_key_exact_count": sum(comparison_key_exact),
        "elapsed_ms": round((time.perf_counter() - started) * 1000),
        "predictions": predictions,
        "open_text_exact": open_text_exact,
        "comparison_key_exact": comparison_key_exact,
    }


def _aggregate(result: dict[str, Any]) -> dict[str, int]:
    return {
        name: result[name]
        for name in (
            "sample_count",
            "open_text_exact_count",
            "comparison_key_exact_count",
            "elapsed_ms",
        )
    }


def run(
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    initializer: Path,
    initializer_manifest_sha256: str,
    pilot: Path,
    pilot_manifest_sha256: str,
    training_input: Path,
    training_input_sha256: str,
    catalog_candidates: Path,
    catalog_candidates_sha256: str,
    result_request: Path,
    result_request_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared_data = _read_regular(preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256)
    prepared = _prepared_manifest(json.loads(prepared_data))
    _verify_prepared_files(preparation, prepared)
    if training_input_sha256 != prepared["training_input_sha256"]:
        raise TrainingReplayError("training input is not bound to the preparation")
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
        raise TrainingReplayError("catalog evaluation input is invalid JSON") from error
    labels = _training_labels(training_raw, training_input_sha256)
    candidate_catalog, _, _ = _load_candidates(candidate_raw)
    if candidate_catalog != prepared["catalog_sha256"]:
        raise TrainingReplayError("candidate catalog differs from the preparation")
    _initializer(initializer, initializer_manifest_sha256, preparation_sha256)
    pilot_record = _pilot(pilot, pilot_manifest_sha256, preparation_sha256)
    if pilot_record.get("initializer_manifest_sha256") != initializer_manifest_sha256:
        raise TrainingReplayError("pilot is not bound to the initializer")
    if pilot_record.get("schema") == "scorepeek-private-title-model-training-pilot-v2" and (
        pilot_record.get("training_input_sha256") != training_input_sha256
        or pilot_record.get("catalog_candidate_artifact_sha256")
        != catalog_candidates_sha256
    ):
        raise TrainingReplayError("pilot catalog evaluation inputs differ from replay")
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise TrainingReplayError("output must be a new absolute directory")

    config_data = _read_regular(
        preparation / "training-config.yml",
        MAX_MANIFEST_BYTES,
        prepared["derived_training_config_sha256"],
    )
    dictionary_data = _read_regular(
        preparation / "dictionary.txt", MAX_MANIFEST_BYTES * 2, prepared["dictionary_sha256"]
    )
    evaluation_rows = prepared_rows(preparation, prepared, "evaluation")
    result_rows, request, result_provenance = _result_rows(
        result_request, result_request_sha256
    )
    base_config = yaml.safe_load(config_data)
    tokens = dictionary_data.decode().splitlines()

    sys.path.insert(0, str(source_root))
    initializer_model, ctc_tokens = _model(base_config, tokens)
    initializer_model.set_state_dict(paddle.load(str(initializer / "initializer.pdparams")))
    pilot_model, pilot_tokens = _model(base_config, tokens)
    pilot_model.set_state_dict(paddle.load(str(pilot / "model.pdparams")))
    if ctc_tokens != pilot_tokens:
        raise TrainingReplayError("initializer and pilot token orders differ")
    presentation_transform_id = pilot_record["recipe"]["presentation_transform_id"]
    trie = CatalogTrie(
        candidate_raw["candidates"], ctc_tokens, prepared["output_timesteps"]
    )
    evaluation_truth = training_truth(evaluation_rows, labels["evaluation"])

    initializer_evaluation, _ = evaluate_catalog(
        initializer_model,
        evaluation_rows,
        evaluation_truth,
        ctc_tokens,
        prepared["model_input_width"],
        trie,
        presentation_transform_id,
    )
    pilot_evaluation, _ = evaluate_catalog(
        pilot_model,
        evaluation_rows,
        evaluation_truth,
        ctc_tokens,
        prepared["model_input_width"],
        trie,
        presentation_transform_id,
    )
    initializer_results = _infer(initializer_model, result_rows, ctc_tokens, prepared["model_input_width"], presentation_transform_id)
    pilot_results = _infer(pilot_model, result_rows, ctc_tokens, prepared["model_input_width"], presentation_transform_id)

    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        record = {
            "schema": REPLAY_SCHEMA,
            "training_preparation_sha256": preparation_sha256,
            "initializer_manifest_sha256": initializer_manifest_sha256,
            "pilot_manifest_sha256": pilot_manifest_sha256,
            "training_input_sha256": training_input_sha256,
            "catalog_candidate_artifact_sha256": catalog_candidates_sha256,
            "selected_steps": pilot_record["selected_steps"],
            "presentation_transform_id": presentation_transform_id,
            "evaluation_list_sha256": prepared["label_file_sha256"]["evaluation"],
            "result_request_sha256": result_request_sha256,
            "result_provenance": result_provenance,
            "provisional": True,
            "accepted_holdout_truth": False,
            "initializer": {
                "evaluation": initializer_evaluation,
                "results": _aggregate(initializer_results),
                "result_predictions": initializer_results["predictions"],
                "result_open_text_exact": initializer_results["open_text_exact"],
                "result_comparison_key_exact": initializer_results["comparison_key_exact"],
            },
            "pilot": {
                "evaluation": pilot_evaluation,
                "results": _aggregate(pilot_results),
                "result_predictions": pilot_results["predictions"],
                "result_open_text_exact": pilot_results["open_text_exact"],
                "result_comparison_key_exact": pilot_results["comparison_key_exact"],
            },
            "evaluation_fully_correct_song_delta": pilot_evaluation[
                "fully_correct_song_count"
            ]
            - initializer_evaluation["fully_correct_song_count"],
            "result_open_text_exact_delta": pilot_results["open_text_exact_count"]
            - initializer_results["open_text_exact_count"],
            "result_comparison_key_exact_delta": pilot_results["comparison_key_exact_count"]
            - initializer_results["comparison_key_exact_count"],
            "result_source_pts": [observation["source_pts"] for observation in request["observations"]],
        }
        (staging / "manifest.json").write_text(
            json.dumps(record, separators=(",", ":"), ensure_ascii=False, allow_nan=False) + "\n",
            encoding="utf-8",
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
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--initializer", type=Path, required=True)
    parser.add_argument("--initializer-manifest-sha256", required=True)
    parser.add_argument("--pilot", type=Path, required=True)
    parser.add_argument("--pilot-manifest-sha256", required=True)
    parser.add_argument("--training-input", type=Path, required=True)
    parser.add_argument("--training-input-sha256", required=True)
    parser.add_argument("--catalog-candidates", type=Path, required=True)
    parser.add_argument("--catalog-candidates-sha256", required=True)
    parser.add_argument("--result-request", type=Path, required=True)
    parser.add_argument("--result-request-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.source_root,
            arguments.initializer,
            arguments.initializer_manifest_sha256,
            arguments.pilot,
            arguments.pilot_manifest_sha256,
            arguments.training_input,
            arguments.training_input_sha256,
            arguments.catalog_candidates,
            arguments.catalog_candidates_sha256,
            arguments.result_request,
            arguments.result_request_sha256,
            arguments.output,
        )
    except Exception as error:
        print(f"scorepeek training replay failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":"), ensure_ascii=False))


if __name__ == "__main__":
    main()
