"""Export a selected private title model through the registered PaddleOCR source."""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any

from scorepeek_ocr.training_artifacts import (
    MAX_MODEL_FILE_BYTES,
    _hash_unpinned_file,
    _prepared_manifest,
)
from scorepeek_ocr.provisional_labels import _valid_sha256
from scorepeek_ocr.training_initializer import (
    MAX_MANIFEST_BYTES,
    TrainingInitializerError,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_process import run_checked
from scorepeek_ocr.training_source import load_registered_source, verify_source
from scorepeek_ocr.title_presentation import IDENTITY_TRANSFORM_ID, TRANSFORM_IDS

EXPORT_SCHEMA = "scorepeek-private-title-model-converted-export-v1"
ONNX_OPSET = 11
EXPORT_TIMEOUT_SECONDS = 10 * 60
CONVERSION_TIMEOUT_SECONDS = 10 * 60


class TrainingExportError(Exception):
    """The selected private title model could not be exported."""


def _catalog_probe_valid(probe: Any, *, candidate: bool) -> bool:
    keys = {
        "sample_count",
        "song_count",
        "fully_correct_song_count",
        "correct_unique_song_id_decision_count",
        "incorrect_or_tied_song_id_decision_count",
        "strict_open_text_count",
        "minimum_correct_runner_up_margin",
        "maximum_incorrect_runner_up_margin",
        "elapsed_ms",
    }
    if candidate:
        keys |= {"steps", "training_sample_count", "training_list_sha256"}
    if not isinstance(probe, dict) or set(probe) != keys:
        return False
    counts = [
        probe[name]
        for name in (
            "sample_count",
            "song_count",
            "fully_correct_song_count",
            "correct_unique_song_id_decision_count",
            "incorrect_or_tied_song_id_decision_count",
            "strict_open_text_count",
            "elapsed_ms",
        )
    ]
    if any(type(value) is not int or value < 0 for value in counts):
        return False
    sample_count = probe["sample_count"]
    if (
        probe["correct_unique_song_id_decision_count"]
        + probe["incorrect_or_tied_song_id_decision_count"]
        != sample_count
        or not 0 <= probe["fully_correct_song_count"] <= probe["song_count"] <= sample_count
        or probe["strict_open_text_count"] > sample_count
    ):
        return False
    for name in (
        "minimum_correct_runner_up_margin",
        "maximum_incorrect_runner_up_margin",
    ):
        value = probe[name]
        if value is not None and (
            type(value) not in (int, float) or not math.isfinite(value) or value < 0
        ):
            return False
    return not candidate or (
        probe["steps"] in (1, 2, 4)
        and type(probe["training_sample_count"]) is int
        and probe["training_sample_count"] == probe["steps"] * 4
        and _valid_sha256(probe["training_list_sha256"])
    )


def _pilot(path: Path, expected_sha256: str, prepared: dict[str, Any]) -> dict[str, Any]:
    data = _read_regular(path / "manifest.json", MAX_MANIFEST_BYTES, expected_sha256)
    try:
        record = json.loads(data)
    except json.JSONDecodeError as error:
        raise TrainingExportError("training pilot manifest is invalid JSON") from error
    recipe = record.get("recipe") if isinstance(record, dict) else None
    schema = record.get("schema") if isinstance(record, dict) else None
    if schema == "scorepeek-private-title-model-training-pilot-v1" and isinstance(
        recipe, dict
    ):
        record = dict(record)
        recipe = dict(recipe)
        recipe.setdefault("presentation_transform_id", IDENTITY_TRANSFORM_ID)
        record["recipe"] = recipe
    v2_fields = {
        "schema",
        "training_preparation_sha256",
        "training_input_sha256",
        "catalog_candidate_artifact_sha256",
        "training_source_commit",
        "initializer_manifest_sha256",
        "initializer_checkpoint",
        "recipe",
        "baseline_probe",
        "candidates",
        "selected_steps",
        "selected_checkpoint",
        "provisional",
        "accepted_holdout_truth",
        "permission_status",
    }
    v2_valid = schema != "scorepeek-private-title-model-training-pilot-v2" or (
        set(record) == v2_fields
        and record.get("training_input_sha256") == prepared["training_input_sha256"]
        and _valid_sha256(record.get("catalog_candidate_artifact_sha256"))
        and _valid_sha256(record.get("initializer_manifest_sha256"))
        and _catalog_probe_valid(record.get("baseline_probe"), candidate=False)
        and isinstance(record.get("candidates"), list)
        and bool(record["candidates"])
        and all(_catalog_probe_valid(item, candidate=True) for item in record["candidates"])
        and [item["steps"] for item in record["candidates"]]
        in ([1], [1, 2], [1, 2, 4])
        and record["candidates"][-1].get("steps") == record.get("selected_steps")
        and record["baseline_probe"]["sample_count"]
        == prepared.get("split_label_counts", {}).get("validation")
        and all(
            item["sample_count"] == record["baseline_probe"]["sample_count"]
            and item["song_count"] == record["baseline_probe"]["song_count"]
            for item in record["candidates"]
        )
        and record["candidates"][-1]["fully_correct_song_count"]
        > record["baseline_probe"]["fully_correct_song_count"]
    )
    if (
        not isinstance(record, dict)
        or schema
        not in {
            "scorepeek-private-title-model-training-pilot-v1",
            "scorepeek-private-title-model-training-pilot-v2",
        }
        or not v2_valid
        or record.get("training_preparation_sha256")
        != prepared["training_preparation_sha256"]
        or not record.get("provisional")
        or record.get("accepted_holdout_truth") is not False
        or record.get("permission_status") != "permission_not_recorded"
        or not isinstance(recipe, dict)
        or recipe.get("presentation_transform_id") not in TRANSFORM_IDS
        or not isinstance(record.get("selected_checkpoint"), dict)
        or set(record["selected_checkpoint"]) != {"sha256", "bytes"}
    ):
        raise TrainingExportError("training pilot manifest values are invalid")
    checkpoint = _read_regular(
        path / "model.pdparams",
        MAX_MODEL_FILE_BYTES,
        record["selected_checkpoint"]["sha256"],
    )
    if len(checkpoint) != record["selected_checkpoint"]["bytes"]:
        raise TrainingExportError("selected checkpoint size mismatched")
    return record


def _verify_export_inputs(preparation: Path, prepared: dict[str, Any]) -> None:
    try:
        _read_regular(
            preparation / "training-config.yml",
            MAX_MANIFEST_BYTES,
            prepared["derived_training_config_sha256"],
        )
        _read_regular(
            preparation / "dictionary.txt",
            MAX_MANIFEST_BYTES * 2,
            prepared["dictionary_sha256"],
        )
    except (KeyError, TrainingInitializerError) as error:
        raise TrainingExportError("training export input binding is invalid") from error


def export(
    preparation: Path,
    preparation_sha256: str,
    source_root: Path,
    pilot: Path,
    pilot_manifest_sha256: str,
    output: Path,
) -> dict[str, Any]:
    prepared_data = _read_regular(
        preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256
    )
    prepared = _prepared_manifest(json.loads(prepared_data))
    _verify_export_inputs(preparation, prepared)
    pilot_record = _pilot(
        pilot,
        pilot_manifest_sha256,
        {**prepared, "training_preparation_sha256": preparation_sha256},
    )
    source = load_registered_source()
    verify_source(source_root, source)
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise TrainingExportError("output must be a new absolute directory")

    with tempfile.TemporaryDirectory(prefix="scorepeek-title-model-export-") as temporary:
        work = Path(temporary)
        environment = os.environ.copy()
        environment["NO_ALBUMENTATIONS_UPDATE"] = "1"
        run_checked(
            [
                sys.executable,
                str(source_root / source.export_entrypoint.path),
                "-c",
                str(preparation / "training-config.yml"),
                "-o",
                f"Global.checkpoints={pilot / 'model.pdparams'}",
                f"Global.save_inference_dir={work}",
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
                "--model_dir",
                str(work),
                "--model_filename",
                "inference.json",
                "--params_filename",
                "inference.pdiparams",
                "--save_file",
                str(onnx_path),
                "--opset_version",
                str(ONNX_OPSET),
                "--enable_auto_update_opset",
                "False",
                "--enable_onnx_checker",
                "True",
                "--optimize_tool",
                "None",
            ],
            timeout_seconds=CONVERSION_TIMEOUT_SECONDS,
        )
        filenames = {
                "paddle_graph": "inference.json",
                "paddle_parameters": "inference.pdiparams",
                "inference_config": "inference.yml",
                "onnx_model": "inference.onnx",
        }
        staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
        try:
            for filename in filenames.values():
                shutil.copyfile(work / filename, staging / filename)
            files = {
                name: _hash_unpinned_file(staging / filename, name)
                for name, filename in filenames.items()
            }
            record = {
                "schema": EXPORT_SCHEMA,
                "training_preparation_sha256": preparation_sha256,
                "training_source_commit": source.commit,
                "pilot_manifest_sha256": pilot_manifest_sha256,
                "selected_checkpoint": pilot_record["selected_checkpoint"],
                "presentation_transform_id": pilot_record["recipe"][
                    "presentation_transform_id"
                ],
                "paddle2onnx_version": "2.1.0",
                "onnx_opset": ONNX_OPSET,
                "onnx_optimization": "none",
                "onnx_checker": True,
                "files": files,
                "provisional": True,
                "distributable": False,
                "accepted_for_runtime": False,
                "permission_status": prepared["permission_status"],
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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preparation", type=Path, required=True)
    parser.add_argument("--preparation-sha256", required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--pilot", type=Path, required=True)
    parser.add_argument("--pilot-manifest-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = export(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.source,
            arguments.pilot,
            arguments.pilot_manifest_sha256,
            arguments.output,
        )
    except Exception as error:
        print(f"scorepeek training export failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    main()
