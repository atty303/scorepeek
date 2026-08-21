"""Cross-tabulate saved official-model census decisions without rerunning ONNX."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tempfile
from collections import Counter
from pathlib import Path
from typing import Any

from scorepeek_ocr.official_census import OBSERVATION_SCHEMA, SCHEMA as CENSUS_SCHEMA
from scorepeek_ocr.training_initializer import (
    TrainingInitializerError,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_inputs import MAX_INPUT_BYTES

SCHEMA = "scorepeek-private-official-onnx-residual-matrix-v1"
STATES = ("correct", "wrong_unique", "unknown_or_tied")


class OfficialResidualMatrixError(Exception):
    """Saved official-model results cannot form a residual matrix."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _load_json(path: Path, digest: str, limit: int, description: str) -> Any:
    try:
        return json.loads(_read_regular(path, limit, digest))
    except json.JSONDecodeError as error:
        raise OfficialResidualMatrixError(f"{description} is invalid JSON") from error


def _strategy(manifest: dict[str, Any], strategy_id: str) -> dict[str, Any]:
    strategies = manifest.get("strategies")
    if not isinstance(strategies, list):
        raise OfficialResidualMatrixError("census strategies are invalid")
    matches = [
        strategy
        for strategy in strategies
        if isinstance(strategy, dict) and strategy.get("strategy_id") == strategy_id
    ]
    if len(matches) != 1:
        raise OfficialResidualMatrixError("requested census strategy is not unique")
    return matches[0]


def _load_model(
    census_path: Path,
    census_sha256: str,
    observations_path: Path,
    observations_sha256: str,
    strategy_id: str,
) -> dict[str, Any]:
    manifest = _load_json(
        census_path, census_sha256, MAX_INPUT_BYTES, "census manifest"
    )
    observations = _load_json(
        observations_path,
        observations_sha256,
        MAX_INPUT_BYTES,
        "saved observations",
    )
    if not isinstance(manifest, dict) or manifest.get("schema") != CENSUS_SCHEMA:
        raise OfficialResidualMatrixError("census manifest schema is invalid")
    if (
        not isinstance(observations, dict)
        or observations.get("schema") != OBSERVATION_SCHEMA
    ):
        raise OfficialResidualMatrixError("saved observation schema is invalid")
    bindings = (
        "training_input_sha256",
        "catalog_candidate_artifact_sha256",
        "model_id",
        "model_sha256",
        "dictionary_sha256",
        "preprocessor_id",
    )
    if (
        manifest.get("observation_sha256") != observations_sha256
        or any(manifest.get(key) != observations.get(key) for key in bindings)
    ):
        raise OfficialResidualMatrixError("census and observation bindings differ")
    rows = observations.get("rows")
    if not isinstance(rows, list) or not rows:
        raise OfficialResidualMatrixError("saved observation rows are invalid")
    row_by_group: dict[str, dict[str, str]] = {}
    for row in rows:
        if (
            not isinstance(row, dict)
            or set(row) != {"group_id", "crop_file_sha256", "decoded_text"}
            or not all(isinstance(row.get(key), str) for key in row)
            or row["group_id"] in row_by_group
        ):
            raise OfficialResidualMatrixError("saved observation rows are invalid")
        row_by_group[row["group_id"]] = row

    strategy = _strategy(manifest, strategy_id)
    overall = strategy.get("overall")
    unrecognized = strategy.get("unrecognized_songs")
    if (
        not isinstance(overall, dict)
        or overall.get("sample_count") != len(rows)
        or not isinstance(overall.get("song_count"), int)
        or not isinstance(unrecognized, list)
    ):
        raise OfficialResidualMatrixError("census strategy summary is invalid")
    failures: dict[str, dict[str, Any]] = {}
    for song in unrecognized:
        if (
            not isinstance(song, dict)
            or not isinstance(song.get("song_id"), str)
            or not isinstance(song.get("title"), str)
            or not isinstance(song.get("failures"), list)
        ):
            raise OfficialResidualMatrixError("census failure song is invalid")
        for failure in song["failures"]:
            group_id = failure.get("group_id") if isinstance(failure, dict) else None
            if (
                not isinstance(group_id, str)
                or group_id in failures
                or group_id not in row_by_group
                or failure.get("crop_file_sha256")
                != row_by_group[group_id]["crop_file_sha256"]
                or failure.get("predicted_song_id") is not None
                and not isinstance(failure.get("predicted_song_id"), str)
            ):
                raise OfficialResidualMatrixError("census failure row is invalid")
            failures[group_id] = {
                **failure,
                "expected_song_id": song["song_id"],
                "title": song["title"],
            }
    wrong_count = sum(
        failure["predicted_song_id"] is not None for failure in failures.values()
    )
    unknown_count = len(failures) - wrong_count
    if (
        overall.get("wrong_unique_song_id_decision_count") != wrong_count
        or overall.get("unknown_or_tied_song_id_decision_count") != unknown_count
    ):
        raise OfficialResidualMatrixError("census failure counts are inconsistent")
    return {
        "manifest": manifest,
        "strategy": strategy,
        "rows": rows,
        "row_by_group": row_by_group,
        "failures": failures,
        "census_sha256": census_sha256,
        "observations_sha256": observations_sha256,
    }


def _state(model: dict[str, Any], group_id: str) -> str:
    failure = model["failures"].get(group_id)
    if failure is None:
        return "correct"
    if failure["predicted_song_id"] is None:
        return "unknown_or_tied"
    return "wrong_unique"


def _model_record(model: dict[str, Any]) -> dict[str, Any]:
    manifest = model["manifest"]
    return {
        "model_id": manifest["model_id"],
        "model_sha256": manifest["model_sha256"],
        "dictionary_sha256": manifest["dictionary_sha256"],
        "preprocessor_id": manifest["preprocessor_id"],
        "census_sha256": model["census_sha256"],
        "observations_sha256": model["observations_sha256"],
    }


def _predicted_song_id(
    state: str, failure: dict[str, Any] | None, expected_song_id: str
) -> str | None:
    if state == "correct":
        return expected_song_id
    if state == "unknown_or_tied":
        return None
    return failure["predicted_song_id"]


def build_matrix(left: dict[str, Any], right: dict[str, Any]) -> dict[str, Any]:
    left_manifest = left["manifest"]
    right_manifest = right["manifest"]
    shared_bindings = (
        "training_input_sha256",
        "catalog_candidate_artifact_sha256",
        "catalog_sha256",
        "comparison_key_id",
        "catalog_song_count",
    )
    if any(left_manifest.get(key) != right_manifest.get(key) for key in shared_bindings):
        raise OfficialResidualMatrixError("model census domains differ")
    left_rows = left["rows"]
    right_rows = right["rows"]
    if [
        (row["group_id"], row["crop_file_sha256"]) for row in left_rows
    ] != [(row["group_id"], row["crop_file_sha256"]) for row in right_rows]:
        raise OfficialResidualMatrixError("model observation rows differ or are reordered")

    cross_table: Counter[tuple[str, str]] = Counter()
    residual_rows = []
    either_unknown_count = 0
    either_unknown_both_empty_count = 0
    both_unknown_count = 0
    both_unknown_both_empty_count = 0
    joint_oracle_failure_song_ids: set[str] = set()
    for left_row, right_row in zip(left_rows, right_rows, strict=True):
        group_id = left_row["group_id"]
        left_state = _state(left, group_id)
        right_state = _state(right, group_id)
        cross_table[left_state, right_state] += 1
        both_empty = not left_row["decoded_text"] and not right_row["decoded_text"]
        either_unknown = "unknown_or_tied" in (left_state, right_state)
        both_unknown = left_state == right_state == "unknown_or_tied"
        either_unknown_count += either_unknown
        either_unknown_both_empty_count += either_unknown and both_empty
        both_unknown_count += both_unknown
        both_unknown_both_empty_count += both_unknown and both_empty
        if left_state == right_state == "correct":
            continue
        left_failure = left["failures"].get(group_id)
        right_failure = right["failures"].get(group_id)
        failure = left_failure or right_failure
        if left_state != "correct" and right_state != "correct":
            joint_oracle_failure_song_ids.add(failure["expected_song_id"])
        residual_rows.append(
            {
                "group_id": group_id,
                "crop_file_sha256": left_row["crop_file_sha256"],
                "expected_song_id": failure["expected_song_id"],
                "title": failure["title"],
                "left": {
                    "state": left_state,
                    "decoded_text": left_row["decoded_text"],
                    "predicted_song_id": _predicted_song_id(
                        left_state, left_failure, failure["expected_song_id"]
                    ),
                },
                "right": {
                    "state": right_state,
                    "decoded_text": right_row["decoded_text"],
                    "predicted_song_id": _predicted_song_id(
                        right_state, right_failure, failure["expected_song_id"]
                    ),
                },
            }
        )
    song_count = left["strategy"]["overall"]["song_count"]
    return {
        "schema": SCHEMA,
        "strategy_id": left["strategy"]["strategy_id"],
        "training_input_sha256": left_manifest["training_input_sha256"],
        "catalog_candidate_artifact_sha256": left_manifest[
            "catalog_candidate_artifact_sha256"
        ],
        "catalog_sha256": left_manifest["catalog_sha256"],
        "comparison_key_id": left_manifest["comparison_key_id"],
        "catalog_song_count": left_manifest["catalog_song_count"],
        "sample_count": len(left_rows),
        "song_count": song_count,
        "left_model": _model_record(left),
        "right_model": _model_record(right),
        "decision_cross_table": [
            {"left_state": left_state, "right_state": right_state, "count": count}
            for left_state in STATES
            for right_state in STATES
            if (count := cross_table[left_state, right_state])
        ],
        "either_unknown_or_tied_decision_count": either_unknown_count,
        "either_unknown_or_tied_with_both_open_text_empty_count": (
            either_unknown_both_empty_count
        ),
        "both_unknown_or_tied_decision_count": both_unknown_count,
        "both_unknown_or_tied_with_both_open_text_empty_count": (
            both_unknown_both_empty_count
        ),
        "joint_oracle_fully_correct_song_count": song_count
        - len(joint_oracle_failure_song_ids),
        "joint_oracle_incomplete_song_ids": sorted(joint_oracle_failure_song_ids),
        "residual_rows": residual_rows,
    }


def run(
    left_census: Path,
    left_census_sha256: str,
    left_observations: Path,
    left_observations_sha256: str,
    right_census: Path,
    right_census_sha256: str,
    right_observations: Path,
    right_observations_sha256: str,
    strategy_id: str,
    output: Path,
) -> dict[str, Any]:
    left = _load_model(
        left_census,
        left_census_sha256,
        left_observations,
        left_observations_sha256,
        strategy_id,
    )
    right = _load_model(
        right_census,
        right_census_sha256,
        right_observations,
        right_observations_sha256,
        strategy_id,
    )
    record = build_matrix(left, right)
    encoded = (
        json.dumps(record, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
        + "\n"
    ).encode()
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        (staging / "manifest.json").write_bytes(encoded)
        _publish(staging, output)
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise
    return {**record, "artifact_sha256": _sha256(encoded)}


def main() -> None:
    parser = argparse.ArgumentParser()
    for side in ("left", "right"):
        parser.add_argument(f"--{side}-census", type=Path, required=True)
        parser.add_argument(f"--{side}-census-sha256", required=True)
        parser.add_argument(f"--{side}-observations", type=Path, required=True)
        parser.add_argument(f"--{side}-observations-sha256", required=True)
    parser.add_argument("--strategy-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.left_census,
            arguments.left_census_sha256,
            arguments.left_observations,
            arguments.left_observations_sha256,
            arguments.right_census,
            arguments.right_census_sha256,
            arguments.right_observations,
            arguments.right_observations_sha256,
            arguments.strategy_id,
            arguments.output,
        )
    except (
        OSError,
        ValueError,
        TrainingInitializerError,
        OfficialResidualMatrixError,
    ) as error:
        print(f"scorepeek official residual matrix failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "output": str(arguments.output),
                "artifact_sha256": result["artifact_sha256"],
                "strategy_id": result["strategy_id"],
                "sample_count": result["sample_count"],
                "song_count": result["song_count"],
                "either_unknown_or_tied_decision_count": result[
                    "either_unknown_or_tied_decision_count"
                ],
                "either_unknown_or_tied_with_both_open_text_empty_count": result[
                    "either_unknown_or_tied_with_both_open_text_empty_count"
                ],
                "both_unknown_or_tied_decision_count": result[
                    "both_unknown_or_tied_decision_count"
                ],
                "both_unknown_or_tied_with_both_open_text_empty_count": result[
                    "both_unknown_or_tied_with_both_open_text_empty_count"
                ],
                "joint_oracle_fully_correct_song_count": result[
                    "joint_oracle_fully_correct_song_count"
                ],
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
