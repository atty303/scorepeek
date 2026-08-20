from __future__ import annotations

import json
import os
import selectors
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scorepeek_ocr.model_store import (
    load_registered_onnx_bundle,
    load_registered_onnx_source,
)
from scorepeek_ocr.official_census import (
    DECODE_SCHEMA,
    DIAGNOSTIC_SCHEMA,
    DYNAMIC_DECODE_SCHEMA,
    OBSERVATION_SCHEMA,
    _DiagnosticRecorder,
    _decode_rows,
    _publish_observations,
    _record_failure,
    run,
    OfficialCensusError,
    _load_observations,
    _model_record,
    _run_bounded,
    _sha256,
    _terminate_process_group,
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

    def test_decoder_cleanup_failure_overrides_the_operation_failure(self) -> None:
        def cleanup_then_report_failure(
            process: subprocess.Popen[bytes], grace: float
        ) -> bool:
            _terminate_process_group(process, grace)
            return False

        with (
            patch(
                "scorepeek_ocr.official_census._terminate_process_group",
                side_effect=cleanup_then_report_failure,
            ),
            self.assertRaisesRegex(
                OfficialCensusError, "resources could not be cleaned up"
            ) as raised,
        ):
            _run_bounded(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                timeout=0.05,
                termination_grace=0.05,
            )
        self.assertEqual(raised.exception.error_type, "decoder_reap")

    def test_decoder_initial_group_cleanup_exception_stays_classified(self) -> None:
        original_terminate = _terminate_process_group
        calls = 0

        def fail_then_cleanup(
            process: subprocess.Popen[bytes], grace: float
        ) -> bool:
            nonlocal calls
            calls += 1
            if calls == 1:
                raise OSError("initial cleanup failed")
            return original_terminate(process, grace)

        child = "import time; time.sleep(30)"
        parent = (
            "import subprocess,sys; "
            "subprocess.Popen("
            f"[sys.executable,'-c',{child!r}],"
            "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,"
            "stderr=subprocess.DEVNULL)"
        )
        with (
            patch(
                "scorepeek_ocr.official_census._terminate_process_group",
                side_effect=fail_then_cleanup,
            ),
            self.assertRaises(OfficialCensusError) as raised,
        ):
            _run_bounded(
                [sys.executable, "-c", parent], termination_grace=0.05
            )
        self.assertEqual(calls, 2)
        self.assertEqual(raised.exception.error_type, "decoder_reap")

    def test_decoder_signal_during_final_group_cleanup_cancels_success(self) -> None:
        original_terminate = _terminate_process_group

        def cleanup_then_interrupt(
            process: subprocess.Popen[bytes], grace: float
        ) -> bool:
            cleaned = original_terminate(process, grace)
            signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
            return cleaned

        child = (
            "import signal,time; "
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
        )
        parent = (
            "import subprocess,sys; "
            "subprocess.Popen("
            f"[sys.executable,'-c',{child!r}],"
            "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,"
            "stderr=subprocess.DEVNULL)"
        )
        with (
            patch(
                "scorepeek_ocr.official_census._terminate_process_group",
                side_effect=cleanup_then_interrupt,
            ),
            self.assertRaisesRegex(OfficialCensusError, "interrupted by signal")
            as raised,
        ):
            _run_bounded(
                [sys.executable, "-c", parent], termination_grace=0.05
            )
        self.assertEqual(raised.exception.status, "cancel")

    def test_decoder_finalizers_all_run_after_selector_close_failure(self) -> None:
        original_popen = subprocess.Popen
        original_close = selectors.EpollSelector.close
        spawned: list[subprocess.Popen[bytes]] = []
        previous_handlers = {
            selected: signal.getsignal(selected) for selected in (signal.SIGINT, signal.SIGTERM)
        }

        def capture_process(
            *args: object, **kwargs: object
        ) -> subprocess.Popen[bytes]:
            process = original_popen(*args, **kwargs)
            spawned.append(process)
            return process

        def close_then_fail(selector: selectors.EpollSelector) -> None:
            original_close(selector)
            raise OSError("selector close failed")

        with (
            patch(
                "scorepeek_ocr.official_census.subprocess.Popen",
                side_effect=capture_process,
            ),
            patch.object(selectors.EpollSelector, "close", close_then_fail),
            self.assertRaisesRegex(
                OfficialCensusError, "resources could not be cleaned up"
            ) as raised,
        ):
            _run_bounded(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                timeout=0.05,
                termination_grace=0.05,
            )
        self.assertEqual(raised.exception.error_type, "decoder_reap")
        self.assertTrue(spawned[0].stdout.closed)
        self.assertTrue(spawned[0].stderr.closed)
        self.assertEqual(
            {selected: signal.getsignal(selected) for selected in previous_handlers},
            previous_handlers,
        )

    def test_decoder_interrupt_kills_parent_and_term_ignoring_descendant(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            pids = Path(temporary) / "pids"
            child = (
                "import signal,time; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
            )
            parent = (
                "import os,pathlib,subprocess,sys,time; "
                f"child=subprocess.Popen([sys.executable,'-c',{child!r}]); "
                f"pathlib.Path({str(pids)!r}).write_text(f'{{os.getpid()}} {{child.pid}}'); "
                "time.sleep(30)"
            )

            def interrupt(*_args: object, **_kwargs: object) -> None:
                time.sleep(0.2)
                raise KeyboardInterrupt

            with patch.object(selectors.EpollSelector, "select", side_effect=interrupt):
                with self.assertRaises(KeyboardInterrupt):
                    _run_bounded(
                        [sys.executable, "-c", parent], termination_grace=0.05
                    )
            parent_pid, child_pid = (int(value) for value in pids.read_text().split())
            deadline = time.monotonic() + 1.0
            while any(Path(f"/proc/{pid}").exists() for pid in (parent_pid, child_pid)):
                if time.monotonic() >= deadline:
                    self.fail("interrupted decoder process group was not removed")
                time.sleep(0.01)

    def test_decoder_handles_signal_during_spawn(self) -> None:
        original_popen = subprocess.Popen
        spawned: list[subprocess.Popen[bytes]] = []

        def spawn_and_interrupt(
            *args: object, **kwargs: object
        ) -> subprocess.Popen[bytes]:
            process = original_popen(*args, **kwargs)
            spawned.append(process)
            signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
            return process

        with (
            patch(
                "scorepeek_ocr.official_census.subprocess.Popen",
                side_effect=spawn_and_interrupt,
            ),
            self.assertRaisesRegex(OfficialCensusError, "interrupted by signal"),
        ):
            _run_bounded(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                termination_grace=0.05,
            )
        self.assertEqual(len(spawned), 1)
        self.assertIsNotNone(spawned[0].poll())

    def test_decoder_cleans_detached_stream_descendant_after_any_exit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            for returncode in (0, 3):
                pid_file = Path(temporary) / f"child-{returncode}"
                child = (
                    "import signal,time; "
                    "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
                )
                parent = (
                    "import pathlib,subprocess,sys; "
                    "child=subprocess.Popen("
                    f"[sys.executable,'-c',{child!r}],"
                    "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,"
                    "stderr=subprocess.DEVNULL); "
                    f"pathlib.Path({str(pid_file)!r}).write_text(str(child.pid)); "
                    f"raise SystemExit({returncode})"
                )
                actual, _, _ = _run_bounded(
                    [sys.executable, "-c", parent], termination_grace=0.05
                )
                self.assertEqual(actual, returncode)
                child_pid = int(pid_file.read_text())
                deadline = time.monotonic() + 1.0
                while Path(f"/proc/{child_pid}").exists():
                    if time.monotonic() >= deadline:
                        self.fail("decoder descendant survived leader exit")
                    time.sleep(0.01)

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

    def test_dynamic_decoder_response_requires_request_and_tensor_bindings(self) -> None:
        source = load_registered_onnx_bundle("pp-ocrv6-tiny-rec-onnx-v1")
        files = {entry.filename: entry.sha256 for entry in source.files}
        response = {
            "schema": DYNAMIC_DECODE_SCHEMA,
            "request_sha256": "1" * 64,
            "model_id": source.model_id,
            "model_sha256": files["inference.onnx"],
            "dictionary_sha256": files["inference.yml"],
            "preprocessor_id": (
                "paddleocr-3.7.0-bgr-dynamic-rec-resize-3x48x320-3200-v1"
            ),
            "elapsed_ms": 1,
            "input_widths": [506],
            "input_tensor_sha256s": ["2" * 64],
            "output_timesteps": [63],
            "decoded_text": ["CAT"],
        }
        from scorepeek_ocr.official_census import _tiny_contract

        validated = _validate_decoded_response(
            response, 1, _tiny_contract(), "1" * 64
        )
        self.assertEqual(validated["decoded_text"], ["CAT"])
        with self.assertRaises(OfficialCensusError):
            _validate_decoded_response(
                {**response, "request_sha256": "3" * 64},
                1,
                _tiny_contract(),
                "1" * 64,
            )

    def test_tiny_decoder_runs_bounded_batches_and_records_progress(self) -> None:
        rows = [(f"/crop/{index}", "group", f"{index:064x}") for index in range(129)]
        source = load_registered_onnx_bundle("pp-ocrv6-tiny-rec-onnx-v1")
        files = {entry.filename: entry.sha256 for entry in source.files}

        def decode(command: list[str]) -> tuple[int, bytes, bytes]:
            request_path = Path(command[command.index("--request") + 1])
            request_bytes = request_path.read_bytes()
            row_count = len(json.loads(request_bytes)["rows"])
            response = {
                "schema": DYNAMIC_DECODE_SCHEMA,
                "request_sha256": _sha256(request_bytes),
                "model_id": source.model_id,
                "model_sha256": files["inference.onnx"],
                "dictionary_sha256": files["inference.yml"],
                "preprocessor_id": (
                    "paddleocr-3.7.0-bgr-dynamic-rec-resize-3x48x320-3200-v1"
                ),
                "elapsed_ms": row_count,
                "input_widths": [320] * row_count,
                "input_tensor_sha256s": ["2" * 64] * row_count,
                "output_timesteps": [40] * row_count,
                "decoded_text": ["CAT"] * row_count,
            }
            return 0, json.dumps(response).encode(), b""

        with tempfile.TemporaryDirectory() as temporary:
            diagnostic = Path(temporary) / "diagnostic"
            recorder = _DiagnosticRecorder(diagnostic, len(rows))
            with patch(
                "scorepeek_ocr.official_census._run_bounded", side_effect=decode
            ) as bounded:
                result = _decode_rows(
                    rows, None, None, Path(temporary) / "bundle", recorder
                )
            self.assertEqual(bounded.call_count, 2)
            self.assertEqual(result["decoded_text"], ["CAT"] * 129)
            self.assertEqual(result["elapsed_ms"], 129)
            record = json.loads((diagnostic / "snapshot.json").read_bytes())
            self.assertEqual(record["schema"], DIAGNOSTIC_SCHEMA)
            self.assertEqual(record["completed_rows"], 129)

    def test_diagnostic_distinguishes_timeout_and_cleanup_cannot_interfere(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            diagnostic = Path(temporary) / "diagnostic"
            recorder = _DiagnosticRecorder(diagnostic, 1)
            try:
                _run_bounded(
                    [sys.executable, "-c", "import time; time.sleep(5)"],
                    timeout=0.05,
                    termination_grace=0.05,
                )
            except OfficialCensusError as error:
                _record_failure(recorder, error)
            record = json.loads((diagnostic / "snapshot.json").read_bytes())
            self.assertEqual(record["status"], "timeout")
            self.assertEqual(record["error_type"], "decoder_timeout")

            failed = Path(temporary) / "failed"
            with (
                patch(
                    "scorepeek_ocr.official_census.os.replace",
                    side_effect=OSError("write failed"),
                ),
                patch(
                    "scorepeek_ocr.official_census.os.unlink",
                    side_effect=OSError("cleanup failed"),
                ),
            ):
                failed_recorder = _DiagnosticRecorder(failed, 1)
            self.assertFalse(failed_recorder.available)

    def test_diagnostic_never_replaces_an_existing_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            model = Path(temporary) / "model.onnx"
            model.write_bytes(b"model bytes")
            recorder = _DiagnosticRecorder(model, 0)
            self.assertFalse(recorder.available)
            self.assertEqual(model.read_bytes(), b"model bytes")

    def test_diagnostic_descriptor_cleanup_failures_never_escape(self) -> None:
        original_close = os.close

        def close_then_fail(descriptor: int) -> None:
            original_close(descriptor)
            raise OSError("injected close failure")

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            invalid = root / "invalid"
            invalid.write_bytes(b"not a directory")
            with patch(
                "scorepeek_ocr.official_census.os.close",
                side_effect=close_then_fail,
            ):
                failed = _DiagnosticRecorder(invalid, 0)
            self.assertFalse(failed.available)

            recorder = _DiagnosticRecorder(root / "diagnostic", 0)
            lock = recorder.lock
            directory = recorder.directory
            assert lock is not None and directory is not None
            with patch(
                "scorepeek_ocr.official_census.fcntl.flock",
                side_effect=OSError("unlock failed"),
            ):
                recorder.close()
            with self.assertRaises(OSError):
                os.fstat(lock)
            with self.assertRaises(OSError):
                os.fstat(directory)

    def test_diagnostic_uuid_failures_never_interfere(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch(
                "scorepeek_ocr.official_census.uuid.uuid4",
                side_effect=OSError("entropy unavailable"),
            ):
                disabled = _DiagnosticRecorder(None, 0)
                failed = _DiagnosticRecorder(root / "failed", 0)
            self.assertFalse(disabled.available)
            self.assertFalse(failed.available)

            update_failed = _DiagnosticRecorder(root / "update-failed", 0)
            with patch(
                "scorepeek_ocr.official_census.uuid.uuid4",
                side_effect=OSError("entropy unavailable"),
            ):
                update_failed.update(completed_rows=1)
            self.assertFalse(update_failed.available)

            recording_failed = _DiagnosticRecorder(root / "recording-failed", 0)
            with patch(
                "scorepeek_ocr.official_census.uuid.uuid4",
                side_effect=OSError("entropy unavailable"),
            ):
                _record_failure(recording_failed, KeyboardInterrupt())
            self.assertFalse(recording_failed.available)

    def test_diagnostic_publication_does_not_claim_a_concurrent_directory(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            diagnostic = root / "diagnostic"

            def collide(
                source_directory: int,
                source: str,
                destination_directory: int,
                destination: str,
            ) -> None:
                del source_directory, source
                os.mkdir(destination, dir_fd=destination_directory)
                claimed = os.open(
                    destination,
                    os.O_RDONLY | os.O_DIRECTORY,
                    dir_fd=destination_directory,
                )
                try:
                    sentinel = os.open(
                        "sentinel",
                        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                        0o600,
                        dir_fd=claimed,
                    )
                    os.close(sentinel)
                finally:
                    os.close(claimed)
                raise FileExistsError("concurrent directory won publication")

            with patch(
                "scorepeek_ocr.official_census._rename_noreplace",
                side_effect=collide,
            ):
                recorder = _DiagnosticRecorder(diagnostic, 0)

            self.assertFalse(recorder.available)
            self.assertEqual(
                {entry.name for entry in diagnostic.iterdir()}, {"sentinel"}
            )
            self.assertFalse(
                any("diagnostic-staging" in entry.name for entry in root.iterdir())
            )

    def test_diagnostic_updates_only_the_claimed_directory_inode(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            diagnostic = root / "diagnostic"
            recorder = _DiagnosticRecorder(diagnostic, 1)
            claimed = root / "claimed-elsewhere"
            diagnostic.rename(claimed)
            diagnostic.write_bytes(b"unrelated bytes")
            recorder.update(completed_rows=1)
            recorder.close()
            self.assertEqual(diagnostic.read_bytes(), b"unrelated bytes")
            record = json.loads((claimed / "snapshot.json").read_bytes())
            self.assertEqual(record["completed_rows"], 1)

    def test_diagnostic_store_keeps_one_locked_latest_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary) / "diagnostic"
            first = _DiagnosticRecorder(store, 1)
            concurrent = _DiagnosticRecorder(store, 2)
            self.assertFalse(concurrent.available)
            first.close()
            (store / ".snapshot-abandoned").write_bytes(b"partial")

            later = _DiagnosticRecorder(store, 3)
            self.assertTrue(later.available)
            later.close()
            record = json.loads((store / "snapshot.json").read_bytes())
            self.assertEqual(record["total_rows"], 3)
            self.assertEqual(
                {entry.name for entry in store.iterdir()},
                {".scorepeek-owner.json", ".writer.lock", "snapshot.json"},
            )

    def test_diagnostic_records_early_validation_failure_and_cancel(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            diagnostic = root / "diagnostic"
            with self.assertRaises(OSError):
                run(
                    root / "missing-preparation",
                    "1" * 64,
                    root / "missing-training.json",
                    "2" * 64,
                    root / "missing-candidates.json",
                    "3" * 64,
                    None,
                    None,
                    None,
                    root / "missing-observations.json",
                    "4" * 64,
                    root / "result",
                    diagnostic,
                )
            record = json.loads((diagnostic / "snapshot.json").read_bytes())
            self.assertEqual(record["status"], "error")
            self.assertEqual(record["error_type"], "unexpected_error")

            cancelled_path = root / "cancelled"
            cancelled = _DiagnosticRecorder(cancelled_path, 0)
            _record_failure(cancelled, KeyboardInterrupt())
            cancel_record = json.loads(
                (cancelled_path / "snapshot.json").read_bytes()
            )
            self.assertEqual(cancel_record["status"], "cancel")
            self.assertEqual(cancel_record["error_type"], "cancel")

    def test_presearch_observations_publish_create_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "observations.json"
            _publish_observations(output, b"first\n")
            self.assertEqual(output.read_bytes(), b"first\n")
            with self.assertRaises(OfficialCensusError):
                _publish_observations(output, b"second\n")
            self.assertEqual(output.read_bytes(), b"first\n")

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
