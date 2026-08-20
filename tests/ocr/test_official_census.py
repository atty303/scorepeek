from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from scorepeek_ocr.model_store import load_registered_onnx_source
from scorepeek_ocr.official_census import (
    DECODE_SCHEMA,
    OBSERVATION_SCHEMA,
    OfficialCensusError,
    _load_observations,
    _model_record,
    _run_bounded,
    _sha256,
    _validate_decoded_response,
)
from scorepeek_ocr.parity import PREPROCESSOR_ID


class OfficialCensusTests(unittest.TestCase):
    def test_decoder_process_is_bounded_by_time_and_both_streams(self) -> None:
        with self.assertRaisesRegex(OfficialCensusError, "timed out"):
            _run_bounded(
                [sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.05
            )
        with self.assertRaisesRegex(OfficialCensusError, "exceeded"):
            _run_bounded(
                [sys.executable, "-c", "print('x' * 1024)"], stdout_limit=8
            )
        with self.assertRaisesRegex(OfficialCensusError, "exceeded"):
            _run_bounded(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stderr.write('x' * 1024)",
                ],
                stderr_limit=8,
            )

    def test_decoder_kills_term_ignoring_descendant_that_holds_pipes(self) -> None:
        child = (
            "import signal,time; "
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(5)"
        )
        parent = (
            "import subprocess,sys,time; "
            f"subprocess.Popen([sys.executable,'-c',{child!r}]); time.sleep(0.1)"
        )
        started = time.monotonic()
        with self.assertRaisesRegex(OfficialCensusError, "timed out"):
            _run_bounded(
                [sys.executable, "-c", parent],
                timeout=0.2,
                termination_grace=0.05,
            )
        self.assertLess(time.monotonic() - started, 1.0)

    def test_cli_normalizes_input_failure_without_a_traceback(self) -> None:
        completed = subprocess.run(
            [
                sys.executable,
                "-m",
                "scorepeek_ocr.official_census",
                "--preparation",
                "/missing",
                "--preparation-sha256",
                "0" * 64,
                "--training-input",
                "/missing",
                "--training-input-sha256",
                "0" * 64,
                "--catalog-candidates",
                "/missing",
                "--catalog-candidates-sha256",
                "0" * 64,
                "--observations",
                "/missing",
                "--observations-sha256",
                "0" * 64,
                "--output",
                "/missing-output",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertIn("scorepeek official ONNX census failed:", completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)

    def test_decoder_response_requires_registered_exact_provenance(self) -> None:
        source = load_registered_onnx_source()
        response = {
            "schema": DECODE_SCHEMA,
            "model_id": source.model_id,
            "model_sha256": source.sha256,
            "dictionary_sha256": source.paddle_inference_yml_sha256,
            "preprocessor_id": PREPROCESSOR_ID,
            "elapsed_ms": 1,
            "decoded_text": ["CAT"],
        }
        self.assertEqual(_validate_decoded_response(response, 1), response)
        with self.assertRaises(OfficialCensusError):
            _validate_decoded_response({**response, "model_sha256": "0" * 64}, 1)
        with self.assertRaises(OfficialCensusError):
            _validate_decoded_response({**response, "extra": True}, 1)

    def test_saved_observations_bind_row_order_and_crop_digest(self) -> None:
        source = load_registered_onnx_source()
        labels = [
            {
                "group_id": "group-1",
                "crop_file_sha256": "1" * 64,
            }
        ]
        record = {
            "schema": OBSERVATION_SCHEMA,
            "training_input_sha256": "2" * 64,
            "catalog_candidate_artifact_sha256": "3" * 64,
            "model_id": source.model_id,
            "model_sha256": source.sha256,
            "dictionary_sha256": source.paddle_inference_yml_sha256,
            "preprocessor_id": PREPROCESSOR_ID,
            "rows": [
                {
                    "group_id": "group-1",
                    "crop_file_sha256": "1" * 64,
                    "decoded_text": "CAT",
                }
            ],
        }
        encoded = (json.dumps(record, separators=(",", ":")) + "\n").encode()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "observations.json"
            path.write_bytes(encoded)
            decoded = _load_observations(
                path, _sha256(encoded), "2" * 64, "3" * 64, labels
            )
            self.assertEqual(decoded["decoded_text"], ["CAT"])
            record["rows"][0]["crop_file_sha256"] = "4" * 64
            changed = (json.dumps(record, separators=(",", ":")) + "\n").encode()
            path.write_bytes(changed)
            with self.assertRaises(OfficialCensusError):
                _load_observations(
                    path, _sha256(changed), "2" * 64, "3" * 64, labels
                )

    def test_distance_search_keeps_unsupported_full_catalog_title(self) -> None:
        songs = [
            ("00000000-0000-0000-0000-000000000001", "CAT"),
            ("00000000-0000-0000-0000-000000000002", "DOG"),
            ("00000000-0000-0000-0000-000000000003", "ZEИITH"),
        ]
        candidates = [
            {
                "song_id": song_id,
                "variants": [{"value": title}],
            }
            for song_id, title in songs
        ]
        labels = [
            {
                "song_id": song_id,
                "title": title,
                "group_id": f"group-{index}",
                "crop_file_sha256": f"{index + 1:064x}",
                "crop_pixel_sha256": f"{index + 11:064x}",
            }
            for index, (song_id, title) in enumerate(songs)
        ]
        records = _model_record(
            ["CAT", "DOG", "ZEITH"],
            candidates,
            [song_id for song_id, _ in songs],
            labels,
            {"train": 3, "validation": 0, "evaluation": 0},
        )
        by_strategy = {record["strategy_id"]: record for record in records}
        self.assertEqual(
            by_strategy["comparison_key_exact"]["overall"]["fully_correct_song_count"],
            2,
        )
        self.assertEqual(
            by_strategy["levenshtein_distance"]["overall"]["fully_correct_song_count"],
            3,
        )
        self.assertEqual(
            by_strategy["levenshtein_distance"][
                "gained_song_ids_vs_comparison_key_exact"
            ],
            [songs[2][0]],
        )
        self.assertEqual(
            by_strategy["levenshtein_normalized_similarity"]["overall"]
            ["fully_correct_song_count"],
            3,
        )

    def test_exact_search_does_not_let_folded_collision_override_exact(self) -> None:
        songs = [
            ("00000000-0000-0000-0000-000000000001", "Ａ"),
            ("00000000-0000-0000-0000-000000000002", "A"),
        ]
        candidates = [
            {"song_id": song_id, "variants": [{"value": title}]}
            for song_id, title in songs
        ]
        labels = [
            {
                "song_id": songs[0][0],
                "title": songs[0][1],
                "group_id": "group-1",
                "crop_file_sha256": "1" * 64,
                "crop_pixel_sha256": "2" * 64,
            }
        ]
        records = _model_record(
            ["Ａ"],
            candidates,
            [songs[0][0]],
            labels,
            {"train": 1, "validation": 0, "evaluation": 0},
        )
        exact = next(
            record
            for record in records
            if record["strategy_id"] == "comparison_key_exact"
        )
        self.assertEqual(exact["overall"]["fully_correct_song_count"], 1)

    def test_equal_distance_is_unknown_instead_of_guessed(self) -> None:
        songs = [
            ("00000000-0000-0000-0000-000000000001", "CAT"),
            ("00000000-0000-0000-0000-000000000002", "BAT"),
        ]
        candidates = [
            {"song_id": song_id, "variants": [{"value": title}]}
            for song_id, title in songs
        ]
        labels = [
            {
                "song_id": songs[0][0],
                "title": songs[0][1],
                "group_id": "group-1",
                "crop_file_sha256": "1" * 64,
                "crop_pixel_sha256": "2" * 64,
            }
        ]
        records = _model_record(
            ["AT"],
            candidates,
            [songs[0][0]],
            labels,
            {"train": 1, "validation": 0, "evaluation": 0},
        )
        by_strategy = {record["strategy_id"]: record for record in records}
        self.assertEqual(
            by_strategy["levenshtein_distance"]["overall"]
            ["incorrect_or_tied_song_id_decision_count"],
            1,
        )
        self.assertEqual(
            by_strategy["levenshtein_distance"]["overall"]
            ["wrong_unique_song_id_decision_count"],
            0,
        )


if __name__ == "__main__":
    unittest.main()
