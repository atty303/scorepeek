from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scorepeek_ocr.official_census import OBSERVATION_SCHEMA, SCHEMA as CENSUS_SCHEMA
from scorepeek_ocr.official_residual_matrix import (
    OfficialResidualMatrixError,
    _load_model,
    build_matrix,
)


def _model(
    model_id: str,
    texts: list[str],
    failures: list[tuple[int, str | None, str, str]],
) -> dict[str, object]:
    rows = [
        {
            "group_id": f"G{index}",
            "crop_file_sha256": f"{index:064x}",
            "decoded_text": text,
        }
        for index, text in enumerate(texts)
    ]
    failure_rows = {}
    unrecognized = []
    for index, predicted, song_id, title in failures:
        failure = {
            "group_id": f"G{index}",
            "crop_file_sha256": f"{index:064x}",
            "predicted_song_id": predicted,
        }
        failure_rows[f"G{index}"] = {
            **failure,
            "expected_song_id": song_id,
            "title": title,
        }
        unrecognized.append(
            {"song_id": song_id, "title": title, "failures": [failure]}
        )
    wrong = sum(predicted is not None for _, predicted, _, _ in failures)
    manifest = {
        "schema": CENSUS_SCHEMA,
        "training_input_sha256": "1" * 64,
        "catalog_candidate_artifact_sha256": "2" * 64,
        "catalog_sha256": "3" * 64,
        "comparison_key_id": "comparison-v1",
        "catalog_song_count": 3,
        "model_id": model_id,
        "model_sha256": "4" * 64,
        "dictionary_sha256": "5" * 64,
        "preprocessor_id": "preprocessor-v1",
    }
    strategy = {
        "strategy_id": "normalized",
        "overall": {
            "sample_count": len(rows),
            "song_count": 3,
            "wrong_unique_song_id_decision_count": wrong,
            "unknown_or_tied_song_id_decision_count": len(failures) - wrong,
        },
        "unrecognized_songs": unrecognized,
    }
    return {
        "manifest": manifest,
        "strategy": strategy,
        "rows": rows,
        "row_by_group": {row["group_id"]: row for row in rows},
        "failures": failure_rows,
        "census_sha256": "6" * 64,
        "observations_sha256": "7" * 64,
    }


class OfficialResidualMatrixTests(unittest.TestCase):
    def test_cross_table_distinguishes_unknown_signal_from_empty_signal(self) -> None:
        left = _model(
            "small",
            ["ok", "", "same", ""],
            [(1, None, "song-1", "I"), (2, None, "song-2", "T")],
        )
        right = _model(
            "medium",
            ["ok", "1", "same", "wrong"],
            [(1, None, "song-1", "I"), (2, None, "song-2", "T")],
        )

        result = build_matrix(left, right)

        self.assertEqual(result["either_unknown_or_tied_decision_count"], 2)
        self.assertEqual(
            result["either_unknown_or_tied_with_both_open_text_empty_count"], 0
        )
        self.assertEqual(result["both_unknown_or_tied_decision_count"], 2)
        self.assertEqual(
            result["both_unknown_or_tied_with_both_open_text_empty_count"], 0
        )
        self.assertEqual(result["joint_oracle_fully_correct_song_count"], 1)
        self.assertEqual(result["joint_oracle_incomplete_song_ids"], ["song-1", "song-2"])
        self.assertEqual(len(result["residual_rows"]), 2)

    def test_joint_oracle_accepts_a_crop_correct_in_either_model(self) -> None:
        left = _model("small", ["a", ""], [(1, None, "song-1", "I")])
        right = _model("medium", ["a", "i"], [])

        result = build_matrix(left, right)

        self.assertEqual(result["joint_oracle_fully_correct_song_count"], 3)
        self.assertEqual(result["joint_oracle_incomplete_song_ids"], [])
        self.assertEqual(result["residual_rows"][0]["right"]["state"], "correct")
        self.assertEqual(
            result["residual_rows"][0]["right"]["predicted_song_id"], "song-1"
        )

    def test_rejects_reordered_observations(self) -> None:
        left = _model("small", ["a", "b"], [])
        right = _model("medium", ["a", "b"], [])
        right["rows"] = list(reversed(right["rows"]))

        with self.assertRaisesRegex(OfficialResidualMatrixError, "reordered"):
            build_matrix(left, right)

    def test_load_rejects_census_observation_digest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            census = root / "census.json"
            observations = root / "observations.json"
            observation_raw = {
                "schema": OBSERVATION_SCHEMA,
                "training_input_sha256": "1" * 64,
                "catalog_candidate_artifact_sha256": "2" * 64,
                "model_id": "model",
                "model_sha256": "3" * 64,
                "dictionary_sha256": "4" * 64,
                "preprocessor_id": "preprocessor",
                "rows": [],
            }
            observation_bytes = json.dumps(observation_raw).encode()
            observations.write_bytes(observation_bytes)
            census_raw = {
                "observation_sha256": "0" * 64,
                **{
                    key: observation_raw[key]
                    for key in observation_raw
                    if key not in {"schema", "rows"}
                },
                "schema": CENSUS_SCHEMA,
                "strategies": [],
            }
            census_bytes = json.dumps(census_raw).encode()
            census.write_bytes(census_bytes)

            with self.assertRaisesRegex(OfficialResidualMatrixError, "bindings differ"):
                _load_model(
                    census,
                    __import__("hashlib").sha256(census_bytes).hexdigest(),
                    observations,
                    __import__("hashlib").sha256(observation_bytes).hexdigest(),
                    "normalized",
                )


if __name__ == "__main__":
    unittest.main()
