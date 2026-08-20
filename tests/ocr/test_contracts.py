from __future__ import annotations

import hashlib
import io
import json
import math
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import cv2
import scorepeek_ocr.model_store as model_store
import numpy as np
import paddle
import unicodedata2 as unicodedata
from scorepeek_ocr.model_store import (
    ModelFile,
    ModelSource,
    OnnxBundleFile,
    OnnxBundleSource,
    OnnxModelSource,
    OnnxNativeContract,
    load_registered_onnx_bundle,
    load_registered_onnx_source,
    load_registered_source,
)
from scorepeek_ocr.parity import ParityError, _canonical_json, ctc_log_probability
from scorepeek_ocr.provisional_labels import (
    CandidateIndex,
    ProvisionalLabelError,
    Variant,
    _associate,
    _comparison_key,
    _exact_comparison_key,
    _load_reviewed_groups,
)
from scorepeek_ocr.spike import (
    CALIBRATED_NORMALIZER_SHA256,
    SpikeError,
    _write_output,
    load_crops,
    load_layout_contract,
)
from scorepeek_ocr.training_inputs import TrainingInputError, generate as generate_training_inputs
from scorepeek_ocr.training_export import (
    TrainingExportError,
    _pilot as load_export_pilot,
)
from scorepeek_ocr.training_artifacts import (
    TrainingArtifactError,
    prepared_rows,
    prepare as prepare_training_artifacts,
    record_export,
)
from scorepeek_ocr.training_catalog import (
    CatalogDecisions,
    CatalogTrie,
    TrainingCatalogError,
    catalog_candidate_sequences,
    improves_catalog_identity,
    training_truth,
)
from scorepeek_ocr.training_census import TrainingCensusError, summarize_songs
from scorepeek_ocr.training_source import (
    TrainingSourceError,
    load_registered_source as load_registered_training_source,
    verify_source as verify_training_source,
)
from scorepeek_ocr.training_initializer import (
    TrainingInitializerError,
    _copy_classes,
    _load_checkpoint_manifest,
    _publish,
    _preprocess,
    _read_regular,
    _tokens,
)
from scorepeek_ocr.training_pilot import (
    TrainingPilotError,
    _config,
    _select_rows,
)
from scorepeek_ocr.training_process import TrainingProcessError, run_checked
from scorepeek_ocr.training_replay import _result_rows
from scorepeek_ocr.title_presentation import (
    CHANNEL_MAX_TRANSFORM_ID,
    IDENTITY_TRANSFORM_ID,
    TitlePresentationError,
    apply_transform,
    transform_crop_bytes,
)


class ContractTests(unittest.TestCase):
    def test_channel_max_title_presentation_is_versioned_and_deterministic(self) -> None:
        image = np.array([[[7, 19, 11], [255, 3, 4]]], dtype=np.uint8)
        transformed = apply_transform(image, CHANNEL_MAX_TRANSFORM_ID)
        np.testing.assert_array_equal(
            transformed,
            np.array([[[19, 19, 19], [255, 255, 255]]], dtype=np.uint8),
        )
        encoded = transform_crop_bytes(
            b"P6\n2 1\n255\n" + image.tobytes(), CHANNEL_MAX_TRANSFORM_ID
        )
        decoded = cv2.imdecode(np.frombuffer(encoded, dtype=np.uint8), cv2.IMREAD_COLOR)
        np.testing.assert_array_equal(decoded, transformed)
        self.assertEqual(CHANNEL_MAX_TRANSFORM_ID, "scorepeek-title-channel-max-rgb-v1")
        self.assertIs(apply_transform(image, IDENTITY_TRANSFORM_ID), image)
        with tempfile.TemporaryDirectory() as temporary:
            crop = Path(temporary) / "crop.ppm"
            crop.write_bytes(b"P6\n2 1\n255\n" + image.tobytes())
            identity_tensor = _preprocess(
                str(crop), 96, presentation_transform_id=IDENTITY_TRANSFORM_ID
            )
            channel_max_tensor = _preprocess(
                str(crop), 96, presentation_transform_id=CHANNEL_MAX_TRANSFORM_ID
            )
            self.assertFalse(np.array_equal(identity_tensor, channel_max_tensor))
            np.testing.assert_array_equal(
                channel_max_tensor[0], channel_max_tensor[1]
            )
            np.testing.assert_array_equal(
                channel_max_tensor[1], channel_max_tensor[2]
            )
        with self.assertRaises(TitlePresentationError):
            apply_transform(np.zeros((1, 1), dtype=np.uint8), IDENTITY_TRANSFORM_ID)
        with self.assertRaises(TitlePresentationError):
            apply_transform(image, "unknown")

    def test_training_process_is_bounded_and_checks_exit_status(self) -> None:
        with self.assertRaises(TrainingProcessError):
            run_checked(
                [sys.executable, "-c", "raise SystemExit(7)"],
                timeout_seconds=5,
            )
        with self.assertRaisesRegex(TrainingProcessError, "timed out"):
            run_checked(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                timeout_seconds=1,
            )

    def test_training_process_cleans_descendants_after_leader_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            child_pid_path = Path(temporary) / "child-pid"
            program = (
                "import pathlib, subprocess; "
                "child = subprocess.Popen(['sleep', '30']); "
                f"pathlib.Path({str(child_pid_path)!r}).write_text(str(child.pid)); "
                "raise SystemExit(7)"
            )
            with self.assertRaisesRegex(TrainingProcessError, "status 7"):
                run_checked([sys.executable, "-c", program], timeout_seconds=5)
            child_pid = int(child_pid_path.read_text())
            deadline = time.monotonic() + 1
            while True:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                if time.monotonic() >= deadline:
                    self.fail("training subprocess descendant survived cleanup")
                time.sleep(0.01)

    def test_training_process_handles_signal_during_spawn(self) -> None:
        original_popen = subprocess.Popen

        def spawn_and_interrupt(*args: object, **kwargs: object) -> subprocess.Popen[bytes]:
            process = original_popen(*args, **kwargs)
            signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
            return process

        with patch(
            "scorepeek_ocr.training_process.subprocess.Popen",
            side_effect=spawn_and_interrupt,
        ), self.assertRaisesRegex(TrainingProcessError, "signal"):
            run_checked(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                timeout_seconds=5,
            )

    def test_training_pilot_selects_nested_rows(self) -> None:
        rows = [
            (f"/crop/{index}.ppm", f"TITLE {index}", f"{index:064x}")
            for index in range(20)
        ]
        one_step = _select_rows(rows, 1)
        two_steps = _select_rows(rows, 2)
        self.assertEqual(len(one_step), 4)
        self.assertEqual(two_steps[:4], one_step)
        with self.assertRaises(TrainingPilotError):
            _select_rows(rows[:3], 1)

        configured = _config(
            {
                "Global": {},
                "Optimizer": {"lr": {}},
                "Train": {"dataset": {}, "sampler": {}, "loader": {}},
                "Eval": {"loader": {}},
            },
            Path("initializer"),
            Path("train.txt"),
            Path("output"),
            1,
            424,
        )
        self.assertEqual(configured["Train"]["sampler"]["scales"], [[424, 48]])

    def test_training_catalog_trie_matches_exhaustive_ctc_and_requires_monotonic_gain(
        self,
    ) -> None:
        trie = CatalogTrie(
            [
                {"song_id": "song-a", "variants": [{"value": "A"}]},
                {"song_id": "song-b", "variants": [{"value": "B"}]},
            ],
            ["blank", "A", "B"],
            3,
            "scorepeek-title-nfc-ucd17-exact-then-ascii-width-fold-v2",
        )
        probabilities = np.asarray(
            [[0.5, 0.4, 0.1], [0.4, 0.5, 0.1], [0.5, 0.4, 0.1]],
            dtype=np.float32,
        )
        scores = trie.score(probabilities)
        self.assertAlmostEqual(scores[0], ctc_log_probability(probabilities, [1]), places=6)
        self.assertAlmostEqual(scores[1], ctc_log_probability(probabilities, [2]), places=6)
        self.assertEqual(trie.expected_indexes(["song-b", "song-a"]), [1, 0])

        truth = ("song-a", "song-b", "song-a")
        predictions = ("song-a", "song-a", "song-a")
        baseline = CatalogDecisions((True, False, True), (3.0, 1.0, 2.0), truth, predictions)
        gain = CatalogDecisions((True, True, True), (2.0, 1.5, 1.0), truth, predictions)
        regression = CatalogDecisions((False, True, True), (1.0, 1.5, 1.0), truth, predictions)
        self.assertTrue(improves_catalog_identity(baseline, gain))
        self.assertFalse(improves_catalog_identity(baseline, regression))
        self.assertFalse(improves_catalog_identity(baseline, baseline))

        partial_truth = ("song-a", "song-a", "song-b")
        partial_baseline = CatalogDecisions(
            (True, False, False),
            (3.0, 1.0, 2.0),
            partial_truth,
            predictions,
        )
        partial_gain = CatalogDecisions(
            (False, True, True),
            (2.0, 1.5, 1.0),
            partial_truth,
            predictions,
        )
        self.assertTrue(improves_catalog_identity(partial_baseline, partial_gain))

        rows = [("/crop/a.ppm", "A", "1" * 64)]
        labels = [{"crop_file_sha256": "1" * 64, "song_id": "song-a"}]
        self.assertEqual(training_truth(rows, labels), ["song-a"])

    def test_training_catalog_uses_only_song_unique_comparison_aliases(self) -> None:
        candidates = [
            {"song_id": "song-pastel", "variants": [{"value": "ＰＡＳＴＥＬＩＳＭ"}]},
            {"song_id": "song-ascii", "variants": [{"value": "A B"}]},
            {"song_id": "song-fullwidth", "variants": [{"value": "ＡＢ"}]},
        ]
        self.assertEqual(
            catalog_candidate_sequences(candidates),
            {
                "song-pastel": ("PASTELISM", "ＰＡＳＴＥＬＩＳＭ"),
                "song-ascii": ("A B", "AB"),
                "song-fullwidth": ("ＡＢ",),
            },
        )
        with self.assertRaises(TrainingCatalogError):
            CatalogTrie(candidates, ["blank", "A", "B", " "], 20, "unknown")

    def test_training_census_summarizes_unrecognized_songs(self) -> None:
        decisions = CatalogDecisions(
            (True, False, True),
            (4.0, 0.25, 3.0),
            ("song-a", "song-a", "song-b"),
            ("song-a", "song-c", "song-b"),
        )
        labels = [
            {
                "group_id": f"group-{index}",
                "song_id": song_id,
                "title": title,
                "crop_file_sha256": f"{index + 1:064x}",
                "crop_pixel_sha256": f"{index + 11:064x}",
            }
            for index, (song_id, title) in enumerate(
                (("song-a", "A"), ("song-a", "A"), ("song-b", "B"))
            )
        ]
        summary, unrecognized = summarize_songs(decisions, labels)
        self.assertEqual(summary["fully_correct_song_count"], 1)
        self.assertEqual(summary["unrecognized_song_count"], 1)
        self.assertEqual(unrecognized[0]["song_id"], "song-a")
        self.assertEqual(unrecognized[0]["failures"][0]["predicted_song_id"], "song-c")
        with self.assertRaises(TrainingCensusError):
            summarize_songs(decisions, labels[:2])

        tied = CatalogDecisions(
            (False,),
            (0.0,),
            ("song-a",),
            (None,),
        )
        _, tied_unrecognized = summarize_songs(tied, labels[:1])
        self.assertIsNone(tied_unrecognized[0]["failures"][0]["predicted_song_id"])

        checkpoint = io.BytesIO()
        paddle.save({"weight": paddle.to_tensor([1.0])}, checkpoint)
        checkpoint.seek(0)
        loaded = paddle.load(checkpoint)
        np.testing.assert_array_equal(loaded["weight"].numpy(), np.array([1.0]))

    def test_export_accepts_legacy_identity_pilot_but_rejects_v2_relabel(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            checkpoint = root / "model.pdparams"
            checkpoint.write_bytes(b"checkpoint")
            checkpoint_record = {
                "sha256": hashlib.sha256(checkpoint.read_bytes()).hexdigest(),
                "bytes": checkpoint.stat().st_size,
            }
            common = {
                "training_preparation_sha256": "1" * 64,
                "provisional": True,
                "accepted_holdout_truth": False,
                "permission_status": "permission_not_recorded",
                "recipe": {},
                "selected_checkpoint": checkpoint_record,
            }
            manifest = root / "manifest.json"
            legacy = {
                "schema": "scorepeek-private-title-model-training-pilot-v1",
                **common,
            }
            manifest.write_text(json.dumps(legacy, separators=(",", ":")) + "\n")
            digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
            prepared = {
                "training_preparation_sha256": "1" * 64,
                "training_input_sha256": "2" * 64,
                "split_label_counts": {"validation": 3},
            }
            loaded = load_export_pilot(root, digest, prepared)
            self.assertEqual(
                loaded["recipe"]["presentation_transform_id"],
                "scorepeek-title-rgb-identity-v1",
            )

            relabelled = {**legacy, "schema": "scorepeek-private-title-model-training-pilot-v2"}
            manifest.write_text(json.dumps(relabelled, separators=(",", ":")) + "\n")
            digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
            with self.assertRaises(TrainingExportError):
                load_export_pilot(root, digest, prepared)

            disguised = {
                **relabelled,
                "training_input_sha256": "2" * 64,
                "catalog_candidate_artifact_sha256": "3" * 64,
                "training_source_commit": "4" * 40,
                "initializer_manifest_sha256": "5" * 64,
                "initializer_checkpoint": checkpoint_record,
                "baseline_probe": {
                    "sample_count": 3,
                    "exact_count": 2,
                    "elapsed_ms": 1,
                },
                "candidates": [
                    {
                        "steps": 1,
                        "training_sample_count": 4,
                        "training_list_sha256": "6" * 64,
                        "sample_count": 3,
                        "exact_count": 3,
                        "elapsed_ms": 1,
                    }
                ],
                "selected_steps": 1,
            }
            manifest.write_text(json.dumps(disguised, separators=(",", ":")) + "\n")
            digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
            with self.assertRaises(TrainingExportError):
                load_export_pilot(root, digest, prepared)

            baseline_probe = {
                "sample_count": 3,
                "song_count": 2,
                "fully_correct_song_count": 1,
                "correct_unique_song_id_decision_count": 2,
                "incorrect_or_tied_song_id_decision_count": 1,
                "strict_open_text_count": 2,
                "minimum_correct_runner_up_margin": 1.0,
                "maximum_incorrect_runner_up_margin": 0.5,
                "elapsed_ms": 1,
            }
            candidate_probe = {
                **baseline_probe,
                "steps": 1,
                "training_sample_count": 4,
                "training_list_sha256": "6" * 64,
                "fully_correct_song_count": 2,
                "correct_unique_song_id_decision_count": 3,
                "incorrect_or_tied_song_id_decision_count": 0,
                "strict_open_text_count": 3,
                "maximum_incorrect_runner_up_margin": None,
            }
            valid_v2 = {
                **disguised,
                "recipe": {
                    "presentation_transform_id": "scorepeek-title-rgb-identity-v1"
                },
                "baseline_probe": baseline_probe,
                "candidates": [candidate_probe],
            }
            manifest.write_text(json.dumps(valid_v2, separators=(",", ":")) + "\n")
            digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
            self.assertEqual(
                load_export_pilot(root, digest, prepared)["schema"],
                "scorepeek-private-title-model-training-pilot-v2",
            )

            mismatched = {
                **valid_v2,
                "candidates": [
                    {
                        **candidate_probe,
                        "sample_count": 4,
                        "song_count": 3,
                        "correct_unique_song_id_decision_count": 4,
                    }
                ],
            }
            manifest.write_text(json.dumps(mismatched, separators=(",", ":")) + "\n")
            digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
            with self.assertRaises(TrainingExportError):
                load_export_pilot(root, digest, prepared)


    def test_registered_pretrained_checkpoint_and_class_mapping(self) -> None:
        source = _load_checkpoint_manifest()
        self.assertEqual(source["bytes"], 124_912_348)
        old_tokens = ["blank", "A", "B", " "]
        new_tokens = ["blank", "A", "B", "Ω", " "]
        source_values = paddle.to_tensor([[0.0, 1.0, 2.0, 3.0]])
        destination = paddle.full([1, 5], -1.0)
        mapped = _copy_classes(destination, source_values, old_tokens, new_tokens, 1)
        np.testing.assert_array_equal(
            mapped.numpy(), np.array([[0.0, 1.0, 2.0, -1.0, 3.0]], dtype=np.float32)
        )

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / "config.yml"
            dictionary = root / "dict.txt"
            target = root / "target.txt"
            config.write_text("Global:\n  character_dict_path: dict.txt\n")
            dictionary.write_text("A\nB\n")
            target.write_text("A\nB\nΩ\n")
            registered = {
                "training_config": {
                    "path": "config.yml", "sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
                },
                "character_dictionary": {
                    "path": "dict.txt", "sha256": hashlib.sha256(dictionary.read_bytes()).hexdigest(),
                },
            }
            self.assertEqual(_tokens(root, registered, target.read_bytes()), (["A", "B"], ["A", "B", "Ω"]))
            dictionary.write_text("B\nA\n")
            with self.assertRaises(TrainingInitializerError):
                _tokens(root, registered, target.read_bytes())

            oversized = root / "oversized.bin"
            oversized.write_bytes(b"x" * 9)
            with self.assertRaises(TrainingInitializerError):
                _read_regular(oversized, 8)
            snapshot = root / "snapshot.bin"
            snapshot.write_bytes(b"registered")
            registered_bytes = _read_regular(
                snapshot, 32, hashlib.sha256(b"registered").hexdigest()
            )
            snapshot.write_bytes(b"replacement")
            self.assertEqual(registered_bytes, b"registered")

    def test_initializer_publication_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            staging = root / "staging"
            output = root / "output"
            staging.mkdir()
            (staging / "artifact").write_bytes(b"complete")
            with patch(
                "scorepeek_ocr.training_initializer._sync_directory",
                side_effect=[None, OSError("injected"), None],
            ), self.assertRaises(OSError):
                _publish(staging, output)
            self.assertFalse(output.exists())

            staging = root / "second-staging"
            staging.mkdir()
            (staging / "artifact").write_bytes(b"replacement")
            output.mkdir()
            marker = output / "marker"
            marker.write_bytes(b"existing")
            with self.assertRaises(FileExistsError):
                _publish(staging, output)
            self.assertEqual(marker.read_bytes(), b"existing")
            self.assertFalse(staging.exists())

    def test_result_replay_rejects_incomplete_crop_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            crop_directory = root / "crops"
            crop_directory.mkdir()
            manifest_data = json.dumps(
                {"schema": "scorepeek-result-crops-v1"}, separators=(",", ":")
            ).encode()
            (crop_directory / "manifest.json").write_bytes(manifest_data)
            manifest_sha256 = hashlib.sha256(manifest_data).hexdigest()
            request_data = json.dumps(
                {
                    "schema": "scorepeek-private-title-model-result-replay-request-v1",
                    "observations": [
                        {
                            "crop_directory": str(crop_directory),
                            "crop_manifest_sha256": manifest_sha256,
                            "expected_title": "TITLE",
                            "source_pts": 1,
                        }
                    ],
                },
                separators=(",", ":"),
            ).encode()
            request_path = root / "request.json"
            request_path.write_bytes(request_data)
            with self.assertRaises(SpikeError):
                _result_rows(request_path, hashlib.sha256(request_data).hexdigest())


    def test_title_model_preparation_requires_complete_dictionary_and_exact_crop_map(self) -> None:
        def write_json(directory: Path, name: str, value: object) -> tuple[Path, str]:
            path = directory / name
            data = json.dumps(value, separators=(",", ":")).encode()
            path.write_bytes(data)
            return path, hashlib.sha256(data).hexdigest()

        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            crop = directory / "crop.ppm"
            pixels = bytes((0, 1, 2, 3, 4, 5))
            crop.write_bytes(b"P6\n2 1\n255\n" + pixels)
            crop_sha = hashlib.sha256(crop.read_bytes()).hexdigest()
            pixel_sha = hashlib.sha256(pixels).hexdigest()
            requirements, requirements_sha = write_json(directory, "requirements.json", {
                "schema": "scorepeek-private-title-model-export-requirements-v1",
                "catalog_sha256": "a" * 64,
                "requirements": {
                    "schema": "scorepeek-title-model-export-requirements-v1",
                    "baseline_dictionary_sha256": "b" * 64,
                    "dictionary_contract_id": "scorepeek-title-unicode-scalar-dictionary-v1",
                    "output_tensor_contract_id": "scorepeek-title-ctc-f32-logits-btc-v1",
                    "ctc_blank_token": 0,
                    "output_timesteps": 40,
                    "output_classes": 5,
                    "baseline_character_count": 2,
                    "appended_catalog_character_count": 1,
                    "non_search_variant_count": 2,
                    "covered_variant_count": 2,
                    "coverage_complete": True,
                    "non_blank_tokens": ["A", "B", "Ω", " "],
                },
            })
            training, training_sha = write_json(directory, "training.json", {
                "schema": "scorepeek-private-title-training-input-manifest-v1",
                "split_contract_id": "scorepeek-title-song-disjoint-sha256-80-10-10-v1",
                "candidate_artifact_sha256": "1" * 64,
                "automated_label_sha256": "2" * 64,
                "visual_audit_sha256": "3" * 64,
                "final_label_sha256": "4" * 64,
                "source_artifact_sha256": "5" * 64,
                "crop_artifact_sha256": "6" * 64,
                "origin": "music_list",
                "permission_status": "permission_not_recorded",
                "provisional": True,
                "accepted_holdout_truth": False,
                "song_count": 1,
                "label_count": 1,
                "split_song_counts": {"train": 1, "validation": 0, "evaluation": 0},
                "splits": {
                    "train": [{
                        "group_id": "G00001",
                        "song_id": "123e4567-e89b-12d3-a456-426614174000",
                        "title": "A B",
                        "crop_pixel_sha256": pixel_sha,
                        "crop_file_sha256": crop_sha,
                        "occurrence_count": 1,
                        "origin": "music_list",
                        "permission_status": "permission_not_recorded",
                    }],
                    "validation": [],
                    "evaluation": [],
                },
            })
            crop_map, crop_map_sha = write_json(directory, "crop-map.json", {
                "schema": "scorepeek-private-title-training-crop-map-v1",
                "training_input_sha256": training_sha,
                "entries": [{
                    "group_id": "G00001",
                    "path": str(crop),
                    "file_sha256": crop_sha,
                    "pixel_sha256": pixel_sha,
                }],
            })
            source = load_registered_training_source()
            output = directory / "prepared #quoted"
            source_config = directory / source.small_rec_config.path
            source_config.parent.mkdir(parents=True)
            source_config.write_text(
                "max_text_length: &max_text_length 25\n"
                "character_dict_path: ppocr/utils/dict/ppocrv6_dict.txt\n"
                "  d2s_train_image_shape: [3, 48, 320]\n"
                "scales: [[320, 32], [320, 48], [320, 64]]\n"
                "image_shape: [48, 320, 3]\n"
                "        image_shape: [3, 48, 320]\n"
                "    data_dir: ./train_data/\n"
                "    - ./train_data/train_list.txt\n"
                "    data_dir: ./train_data\n"
                "    - ./train_data/val_list.txt\n"
            )
            with patch("scorepeek_ocr.training_artifacts.verify_source"):
                summary = prepare_training_artifacts(
                    requirements, requirements_sha, training, training_sha,
                    crop_map, crop_map_sha, directory, output,
                )
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertTrue(summary["coverage_complete"])
            self.assertEqual((output / "dictionary.txt").read_text(), "A\nB\nΩ\n")
            self.assertEqual((output / "train.txt").read_text(), f"{crop}\tA B\n")
            self.assertEqual(
                prepared_rows(output, manifest, "train"),
                [(str(crop), "A B", crop_sha)],
            )
            evidence = json.loads((output / "train-crop-evidence.json").read_text())
            self.assertEqual(evidence["rows"][0]["file_sha256"], crop_sha)
            self.assertEqual(manifest["output_classes"], 5)
            self.assertEqual(manifest["model_input_width"], 320)
            derived_config = (output / "training-config.yml").read_text()
            self.assertIn(
                f"character_dict_path: {json.dumps(str(output / 'dictionary.txt'))}",
                derived_config,
            )
            self.assertIn(f"- {json.dumps(str(output / 'train.txt'))}", derived_config)
            self.assertIn("max_text_length: &max_text_length 40", derived_config)
            self.assertIn("d2s_train_image_shape: [3, 48, 320]", derived_config)
            self.assertIn("scales: [[320, 32], [320, 48], [320, 64]]", derived_config)
            self.assertIn("image_shape: [48, 320, 3]", derived_config)
            self.assertIn("image_shape: [3, 48, 320]", derived_config)
            self.assertFalse(manifest["accepted_holdout_truth"])

            bad = json.loads(requirements.read_text())
            bad["requirements"]["coverage_complete"] = False
            bad_requirements, bad_sha = write_json(directory, "bad-requirements.json", bad)
            with patch("scorepeek_ocr.training_artifacts.verify_source"), self.assertRaises(
                TrainingArtifactError
            ):
                prepare_training_artifacts(
                    bad_requirements, bad_sha, training, training_sha,
                    crop_map, crop_map_sha, directory, directory / "bad-output",
                )

            bad_path_map = json.loads(crop_map.read_text())
            bad_path_map["entries"][0]["path"] = f"{crop}\tinvalid"
            bad_path_map_path, bad_path_map_sha = write_json(
                directory, "bad-path-map.json", bad_path_map
            )
            with patch("scorepeek_ocr.training_artifacts.verify_source"), self.assertRaises(
                TrainingArtifactError
            ):
                prepare_training_artifacts(
                    requirements, requirements_sha, training, training_sha,
                    bad_path_map_path, bad_path_map_sha, directory,
                    directory / "bad-path-output",
                )

            moved = json.loads(training.read_text())
            moved["split_song_counts"] = {"train": 0, "validation": 1, "evaluation": 0}
            moved["splits"]["validation"] = moved["splits"]["train"]
            moved["splits"]["train"] = []
            moved_path, moved_sha = write_json(directory, "moved-training.json", moved)
            moved_crop_map = json.loads(crop_map.read_text())
            moved_crop_map["training_input_sha256"] = moved_sha
            moved_map_path, moved_map_sha = write_json(
                directory, "moved-crop-map.json", moved_crop_map
            )
            with patch("scorepeek_ocr.training_artifacts.verify_source"), self.assertRaises(
                TrainingArtifactError
            ):
                prepare_training_artifacts(
                    requirements, requirements_sha, moved_path, moved_sha,
                    moved_map_path, moved_map_sha, directory, directory / "moved-output",
                )

            failed_output = directory / "failed-output"
            with patch("scorepeek_ocr.training_artifacts.verify_source"), patch(
                "scorepeek_ocr.training_artifacts._sync_directory",
                side_effect=[None, OSError("parent fsync failed"), None],
            ), self.assertRaises(OSError):
                prepare_training_artifacts(
                    requirements, requirements_sha, training, training_sha,
                    crop_map, crop_map_sha, directory, failed_output,
                )
            self.assertFalse(failed_output.exists())

            paddle = directory / "model.pdiparams"
            onnx = directory / "model.onnx"
            paddle.write_bytes(b"paddle")
            onnx.write_bytes(b"onnx")
            export = directory / "export.json"
            record_export(output, summary["manifest_sha256"], paddle, onnx, export)
            export_record = json.loads(export.read_text())
            self.assertFalse(export_record["distributable"])
            self.assertFalse(export_record["accepted_for_runtime"])
            self.assertEqual(export_record["required_output_classes"], 5)
            self.assertFalse(export_record["model_shape_verified"])

            (output / "dictionary.txt").write_text("B\nA\nΩ\n")
            with self.assertRaises(TrainingArtifactError):
                record_export(
                    output, summary["manifest_sha256"], paddle, onnx,
                    directory / "tampered-export.json",
                )

    def test_registered_training_source_requires_the_pinned_checkout_and_files(self) -> None:
        source = load_registered_training_source()
        self.assertEqual(source.commit, "b03f46425e8ff4442b268ce449e3eef758146cd4")
        self.assertEqual(source.license_id, "Apache-2.0")
        self.assertEqual(
            [item.path for item in (
                source.training_entrypoint, source.export_entrypoint,
                source.small_rec_config, source.requirements,
            )],
            [
                "tools/train.py", "tools/export_model.py",
                "configs/rec/PP-OCRv6/PP-OCRv6_small_rec.yml", "requirements.txt",
            ],
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            with patch("scorepeek_ocr.training_source._head", return_value=source.commit), patch(
                "scorepeek_ocr.training_source._sha256_regular_file",
                side_effect=[item.sha256 for item in (
                    source.training_entrypoint, source.export_entrypoint,
                    source.small_rec_config, source.requirements,
                )],
            ):
                verify_training_source(root, source)
            with patch("scorepeek_ocr.training_source._head", return_value="0" * 40):
                with self.assertRaises(TrainingSourceError):
                    verify_training_source(root, source)

    def test_training_inputs_are_song_disjoint_and_bind_every_private_artifact(self) -> None:
        def write(directory: Path, name: str, value: object) -> tuple[Path, str]:
            path = directory / name
            data = json.dumps(value, separators=(",", ":")).encode()
            path.write_bytes(data)
            return path, hashlib.sha256(data).hexdigest()

        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            candidate, candidate_sha = write(directory, "candidate.json", {
                "schema": "scorepeek-private-provisional-title-candidates-v1"
            })
            automated, automated_sha = write(directory, "automated.json", {
                "schema": "scorepeek-private-provisional-music-list-title-labels-v1"
            })
            audit, audit_sha = write(directory, "audit.json", {"schema": "audit"})
            source, source_sha = write(directory, "source.json", {"schema": "source"})
            crops, crops_sha = write(directory, "crops.json", {"schema": "crops"})
            first_song = "123e4567-e89b-12d3-a456-426614174000"
            second_song = "123e4567-e89b-12d3-a456-426614174001"
            final, final_sha = write(directory, "final.json", {
                "schema": "scorepeek-private-final-music-list-title-labels-v1",
                "candidate_artifact_sha256": candidate_sha,
                "automated_label_sha256": automated_sha,
                "visual_audit_sha256": audit_sha,
                "source_artifact_sha256": source_sha,
                "crop_artifact_sha256": crops_sha,
                "labels": [
                    {"group_id": "G00001", "crop_pixel_sha256": "1" * 64,
                     "crop_file_sha256": "2" * 64, "occurrence_count": 2,
                     "song_id": first_song, "title": "A", "origin": "music_list",
                     "permission_status": "permission_not_recorded"},
                    {"group_id": "G00002", "crop_pixel_sha256": "3" * 64,
                     "crop_file_sha256": "4" * 64, "occurrence_count": 1,
                     "song_id": first_song, "title": "A", "origin": "music_list",
                     "permission_status": "permission_not_recorded"},
                    {"group_id": "G00003", "crop_pixel_sha256": "5" * 64,
                     "crop_file_sha256": "6" * 64, "occurrence_count": 1,
                     "song_id": second_song, "title": "B", "origin": "music_list",
                     "permission_status": "permission_not_recorded"},
                ],
            })
            manifest = generate_training_inputs(
                candidate, candidate_sha, automated, automated_sha, audit, audit_sha,
                final, final_sha, source, source_sha, crops, crops_sha,
            )
            songs = {}
            for split, labels in manifest["splits"].items():
                for label in labels:
                    previous = songs.setdefault(label["song_id"], split)
                    self.assertEqual(previous, split)
            self.assertIn(first_song, songs)
            self.assertEqual(manifest["label_count"], 3)
            self.assertTrue(manifest["provisional"])
            self.assertFalse(manifest["accepted_holdout_truth"])

            raw = json.loads(final.read_text())
            raw["crop_artifact_sha256"] = "0" * 64
            final.write_text(json.dumps(raw, separators=(",", ":")))
            with self.assertRaises(TrainingInputError):
                generate_training_inputs(
                    candidate, candidate_sha, automated, automated_sha, audit, audit_sha,
                    final, hashlib.sha256(final.read_bytes()).hexdigest(), source, source_sha,
                    crops, crops_sha,
                )

    def test_comparison_keys_preserve_exact_tier_and_bound_ascii_width_fallback(
        self,
    ) -> None:
        self.assertEqual(unicodedata.unidata_version, "17.0.0")
        self.assertEqual(_exact_comparison_key("ABSOLUTE EVIL"), "ABSOLUTEEVIL")
        self.assertEqual(_exact_comparison_key("ＰＡＳＴＥＬＩＳＭ"), "ＰＡＳＴＥＬＩＳＭ")
        self.assertEqual(_comparison_key("Cafe\N{COMBINING ACUTE ACCENT} Noir"), "CaféNoir")
        self.assertEqual(_comparison_key("ＰＡＳＴＥＬＩＳＭ"), "PASTELISM")
        self.assertEqual(_comparison_key("Ａ！　Ｂ～"), "A!B~")
        self.assertEqual(_comparison_key("Absolute\tEvil"), "Absolute\tEvil")
        self.assertEqual(
            _comparison_key("Absolute\N{NO-BREAK SPACE}Evil"),
            "Absolute\N{NO-BREAK SPACE}Evil",
        )
        self.assertEqual(_comparison_key("Ⅰ①ｶ"), "Ⅰ①ｶ")
        self.assertEqual(
            _comparison_key("a\u0897\u0316"),
            _comparison_key("a\u0316\u0897"),
        )
        self.assertEqual(
            _comparison_key("".join(chr(codepoint) for codepoint in range(0xFF01, 0xFF5F))),
            "".join(chr(codepoint) for codepoint in range(0x21, 0x7F)),
        )
        self.assertNotEqual(
            _comparison_key("ABSOLUTE EVIL"),
            _comparison_key("Absolute Evil"),
        )

    def test_provisional_association_is_exact_and_fail_closed(self) -> None:
        first = Variant("song-a", "ABSOLUTE EVIL", "in_game_display", ("tachi", "r", "d"))
        same = Variant("song-a", "ABSOLUTE EVIL", "official_display", ("textage", "r", "d"))
        spaced = Variant("song-a", "ABSOLUTE  EVIL", "alternate_display", ("tachi", "r", "d"))
        collision = Variant("song-b", "ABSOLUTEEVIL", "in_game_display", ("tachi", "r", "d"))
        key = _exact_comparison_key("ABSOLUTE EVIL")

        def candidates(*variants: Variant) -> CandidateIndex:
            values = list(variants)
            return CandidateIndex({key: values}, {key: values})

        self.assertEqual(
            _associate("ABSOLUTE EVIL", 0.949, candidates(first))[0],
            "low_confidence",
        )
        self.assertEqual(
            _associate("absolute evil", 1.0, candidates(first))[0],
            "no_exact_catalog_candidate",
        )
        self.assertEqual(
            _associate("ABSOLUTE EVIL", 1.0, candidates(first, same))[0],
            "unique",
        )
        self.assertEqual(
            _associate("ABSOLUTEEVIL", 1.0, candidates(first, spaced))[0],
            "ambiguous_display_text",
        )
        self.assertEqual(
            _associate("ABSOLUTE EVIL", 1.0, candidates(first, spaced))[0],
            "ambiguous_display_text",
        )
        self.assertEqual(
            _associate("ABSOLUTE EVIL", 1.0, candidates(first, collision))[0],
            "ambiguous_catalog_songs",
        )

        pastel = Variant(
            "song-pastel",
            "ＰＡＳＴＥＬＩＳＭ",
            "in_game_display",
            ("tachi", "r", "d"),
        )
        pastel_key = _comparison_key(pastel.value)
        pastel_candidates = CandidateIndex({}, {pastel_key: [pastel]})
        self.assertEqual(_associate("PASTELISM", 1.0, pastel_candidates)[0], "unique")
        ascii_pastel = Variant(
            "song-pastel",
            "PASTELISM",
            "alternate_display",
            ("tachi", "r", "d"),
        )
        self.assertEqual(
            _associate(
                "PASTELISM",
                1.0,
                CandidateIndex({}, {pastel_key: [pastel, ascii_pastel]}),
            )[0],
            "ambiguous_display_text",
        )

        exact_first = CandidateIndex(
            {
                _exact_comparison_key("A!"): [
                    Variant("song-a", "A!", "in_game_display", ("tachi", "r", "d"))
                ]
            },
            {
                _comparison_key("A!"): [
                    Variant("song-a", "A!", "in_game_display", ("tachi", "r", "d")),
                    Variant("song-b", "Ａ！", "in_game_display", ("tachi", "r", "d")),
                ]
            },
        )
        self.assertEqual(_associate("A!", 1.0, exact_first)[0], "unique")

        unicode_17_first = Variant(
            "song-a",
            "a\u0897\u0316",
            "in_game_display",
            ("tachi", "r", "d"),
        )
        unicode_17_second = Variant(
            "song-b",
            "a\u0316\u0897",
            "in_game_display",
            ("tachi", "r", "d"),
        )
        unicode_17_key = _exact_comparison_key(unicode_17_first.value)
        unicode_17_candidates = CandidateIndex(
            {
                unicode_17_key: [unicode_17_first, unicode_17_second],
            },
            {
                unicode_17_key: [unicode_17_first, unicode_17_second],
            },
        )
        state, _ = _associate(
            unicode_17_first.value,
            1.0,
            unicode_17_candidates,
        )
        self.assertEqual(state, "ambiguous_catalog_songs")

    def test_provisional_review_groups_require_only_stationary_occurrences(self) -> None:
        digest = "1" * 64
        occurrence = {
            "pair_motion": {"state": "stationary"},
            "crop_file_sha256": "2" * 64,
            "crop_path": "/private/crop.ppm",
        }
        group = {"crop_pixel_sha256": "3" * 64, "occurrences": [occurrence]}
        decision = {
            "group_id": "G00000",
            "crop_pixel_sha256": "3" * 64,
            "occurrence_count": 1,
            "status": "decided",
            "annotation": {
                "content": "title",
                "presentation": {"availability": "available", "color_domain": "standard"},
            },
        }
        plan = {
            "schema": "scorepeek-private-music-list-motion-review-plan-v1",
            "source_artifact_sha256": digest,
            "catalog_sha256": "4" * 64,
            "groups": [group],
        }
        disposition = {
            "schema": "scorepeek-private-music-list-motion-review-disposition-v1",
            "source_review_plan_sha256": "5" * 64,
            "dispositions": [decision],
        }
        source = {
            "schema": "scorepeek-private-music-list-motion-artifact-v1",
            "catalog_sha256": "4" * 64,
        }
        selected = _load_reviewed_groups(
            disposition, plan, source, "6" * 64, "5" * 64, digest, 1
        )[1]
        self.assertEqual(len(selected), 1)

        occurrence["pair_motion"]["state"] = "scrolling"
        with self.assertRaises(ProvisionalLabelError):
            _load_reviewed_groups(
                disposition, plan, source, "6" * 64, "5" * 64, digest, 1
            )

        occurrence["pair_motion"]["state"] = "stationary"
        decision["occurrence_count"] = 2
        with self.assertRaises(ProvisionalLabelError):
            _load_reviewed_groups(
                disposition, plan, source, "6" * 64, "5" * 64, digest, 1
            )

        decision["occurrence_count"] = True
        with self.assertRaises(ProvisionalLabelError):
            _load_reviewed_groups(
                disposition, plan, source, "6" * 64, "5" * 64, digest, 1
            )

    def test_ocr_output_publication_is_create_only_and_cleans_failed_staging(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            output = directory / "ocr.json"
            _write_output(output, '{"complete":true}\n')
            self.assertEqual(output.read_text(), '{"complete":true}\n')
            with self.assertRaises(SpikeError):
                _write_output(output, '{"complete":false}\n')
            self.assertEqual(output.read_text(), '{"complete":true}\n')

            failed = directory / "failed.json"
            with patch("scorepeek_ocr.spike.os.link", side_effect=OSError("full")):
                with self.assertRaises(SpikeError):
                    _write_output(failed, '{"complete":true}\n')
            self.assertFalse(failed.exists())
            self.assertEqual(
                [path.name for path in directory.iterdir()],
                ["ocr.json"],
            )

            unsynced = directory / "unsynced.json"
            with patch(
                "scorepeek_ocr.spike._sync_directory",
                side_effect=[OSError("sync"), None],
            ):
                with self.assertRaises(SpikeError):
                    _write_output(unsynced, '{"complete":true}\n')
            self.assertFalse(unsynced.exists())
            self.assertEqual(
                [path.name for path in directory.iterdir()],
                ["ocr.json"],
            )

    def test_registered_model_manifest_is_exact(self) -> None:
        manifest = (
            Path(__file__).parents[2]
            / "models"
            / "manifests"
            / "pp-ocrv6-small-rec-v1.json"
        )
        with patch.object(
            model_store,
            "_read_regular_bytes",
            wraps=model_store._read_regular_bytes,
        ) as read_manifest:
            source = load_registered_source()
        read_manifest.assert_called_once_with(
            model_store.REGISTERED_MODEL_MANIFEST,
            model_store.MAX_MANIFEST_BYTES,
        )
        self.assertEqual(source.model_name, "PP-OCRv6_small_rec")
        self.assertEqual(
            source.archive_sha256,
            "da460f968ce9f88325ac3a34fa302077d6e9b0dcefb16ba3137cd7796f879d06",
        )
        self.assertEqual(len(source.files), 3)
        rejected = subprocess.run(
            [
                sys.executable,
                "-m",
                "scorepeek_ocr.model_store",
                "fetch",
                "--manifest",
                str(manifest),
            ],
            capture_output=True,
            check=False,
        )
        self.assertEqual(rejected.returncode, 2)

    def test_registered_onnx_model_manifest_is_exact(self) -> None:
        source = load_registered_onnx_source()
        self.assertEqual(source.model_id, "pp-ocrv6-small-rec-onnx-v1")
        self.assertEqual(source.bytes, 21_159_378)
        self.assertEqual(
            source.sha256,
            "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        )

    def test_registered_tiny_onnx_bundle_is_exact(self) -> None:
        source = load_registered_onnx_bundle("pp-ocrv6-tiny-rec-onnx-v1")
        self.assertEqual(source.model_name, "PP-OCRv6_tiny_rec")
        self.assertEqual(source.native_contract.input_color_order, "BGR")
        self.assertEqual(source.native_contract.input_height, 48)
        self.assertEqual(source.native_contract.preprocessor_minimum_width, 320)
        self.assertEqual(source.native_contract.preprocessor_maximum_width, 3200)
        self.assertEqual(source.native_contract.output_classes, 6906)
        self.assertEqual(
            {item.filename for item in source.files},
            {"inference.onnx", "inference.json", "inference.yml"},
        )
        onnx = next(item for item in source.files if item.filename == "inference.onnx")
        self.assertEqual(onnx.bytes, 4_462_639)
        self.assertEqual(
            onnx.sha256,
            "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6",
        )

    def test_registered_medium_onnx_bundle_is_exact(self) -> None:
        source = load_registered_onnx_bundle("pp-ocrv6-medium-rec-onnx-v1")
        self.assertEqual(source.model_name, "PP-OCRv6_medium_rec")
        self.assertEqual(source.native_contract.output_classes, 18_710)
        onnx = next(item for item in source.files if item.filename == "inference.onnx")
        self.assertEqual(onnx.bytes, 76_554_979)
        self.assertEqual(
            onnx.sha256,
            "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
        )

    def test_unregistered_onnx_bundle_is_rejected(self) -> None:
        with self.assertRaises(model_store.ModelStoreError):
            load_registered_onnx_bundle("not-registered")

    def test_onnx_bundle_verification_binds_complete_file_set(self) -> None:
        contents = {
            "inference.onnx": b"onnx",
            "inference.json": b"json",
            "inference.yml": b"yml",
        }
        source = OnnxBundleSource(
            manifest_sha256="1" * 64,
            model_id="test",
            model_name="test",
            source_repository="test/test",
            source_revision="2" * 40,
            native_contract=OnnxNativeContract("NCHW", "BGR", 3, 48, 320, 3200, 4, 0),
            files=tuple(
                OnnxBundleFile(
                    filename=filename,
                    source_url=f"https://example.invalid/{filename}",
                    sha256=hashlib.sha256(data).hexdigest(),
                    bytes=len(data),
                )
                for filename, data in contents.items()
            ),
        )
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for filename, data in contents.items():
                (directory / filename).write_bytes(data)
            model_store.verify_onnx_bundle(directory, source)
            (directory / "unexpected").write_bytes(b"x")
            with self.assertRaises(model_store.ModelStoreError):
                model_store.verify_onnx_bundle(directory, source)

    def test_onnx_bundle_store_recovers_owned_staging_and_bounds_capacity(self) -> None:
        contents = {
            "inference.onnx": b"onnx",
            "inference.json": b"json",
            "inference.yml": b"yml",
        }

        def bundle(manifest_sha256: str) -> OnnxBundleSource:
            return OnnxBundleSource(
                manifest_sha256=manifest_sha256,
                model_id="test",
                model_name="test",
                source_repository="test/test",
                source_revision="2" * 40,
                native_contract=OnnxNativeContract(
                    "NCHW", "BGR", 3, 48, 320, 3200, 4, 0
                ),
                files=tuple(
                    OnnxBundleFile(
                        filename=filename,
                        source_url=f"https://example.invalid/{filename}",
                        sha256=hashlib.sha256(data).hexdigest(),
                        bytes=len(data),
                    )
                    for filename, data in contents.items()
                ),
            )

        def download(item: OnnxBundleFile, path: Path) -> None:
            path.write_bytes(contents[item.filename])

        first = bundle("1" * 64)
        second = bundle("2" * 64)
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary) / "store"
            with (
                patch.object(
                    model_store, "load_registered_onnx_bundle", return_value=first
                ),
                patch.object(
                    model_store, "_download_onnx_bundle_file", side_effect=download
                ),
            ):
                published = model_store.fetch_onnx_bundle(store, first.model_id)
            self.assertFalse(published["reused"])
            bundles = store / "bundles"
            abandoned = bundles / f"{model_store.ONNX_BUNDLE_STAGING_PREFIX}abandoned"
            abandoned.mkdir()
            # This is the crash window immediately after mkdtemp and before the
            # per-run marker. The pre-existing store marker makes it recoverable.
            with patch.object(
                model_store, "load_registered_onnx_bundle", return_value=first
            ):
                recovered = model_store.fetch_onnx_bundle(store, first.model_id)
            self.assertTrue(recovered["reused"])
            self.assertFalse(abandoned.exists())
            self.assertNotIn(
                model_store.ONNX_BUNDLE_STAGING_MARKER,
                {entry.name for entry in (bundles / first.manifest_sha256).iterdir()},
            )

            with (
                patch.object(
                    model_store, "load_registered_onnx_bundle", return_value=second
                ),
                patch.object(model_store, "MAX_ONNX_BUNDLE_COUNT", 1),
                patch.object(
                    model_store,
                    "_download_onnx_bundle_file",
                    side_effect=AssertionError("capacity must fail before download"),
                ),
                self.assertRaises(model_store.ModelStoreError),
            ):
                model_store.fetch_onnx_bundle(store, second.model_id)

            with (
                patch.object(
                    model_store, "load_registered_onnx_bundle", return_value=first
                ),
                patch.object(model_store, "MAX_ONNX_BUNDLE_COUNT", 1),
            ):
                reused = model_store.fetch_onnx_bundle(store, first.model_id)
            self.assertTrue(reused["reused"])

    def test_onnx_bundle_download_failure_cleans_owned_staging(self) -> None:
        data = b"model"
        source = OnnxBundleSource(
            manifest_sha256="3" * 64,
            model_id="test",
            model_name="test",
            source_repository="test/test",
            source_revision="4" * 40,
            native_contract=OnnxNativeContract("NCHW", "BGR", 3, 48, 320, 3200, 4, 0),
            files=(
                OnnxBundleFile(
                    filename="inference.onnx",
                    source_url="https://example.invalid/inference.onnx",
                    sha256=hashlib.sha256(data).hexdigest(),
                    bytes=len(data),
                ),
            ),
        )
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary) / "store"
            with (
                patch.object(
                    model_store, "load_registered_onnx_bundle", return_value=source
                ),
                patch.object(
                    model_store,
                    "_download_onnx_bundle_file",
                    side_effect=model_store.ModelStoreError("injected failure"),
                ),
                self.assertRaises(model_store.ModelStoreError),
            ):
                model_store.fetch_onnx_bundle(store, source.model_id)
            self.assertEqual(
                {entry.name for entry in (store / "bundles").iterdir()},
                {model_store.ONNX_BUNDLE_STORE_MARKER},
            )

    def test_existing_unmarked_bundle_store_is_not_claimed_or_recovered(self) -> None:
        source = load_registered_onnx_bundle("pp-ocrv6-tiny-rec-onnx-v1")
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary) / "store"
            abandoned = (
                store
                / "bundles"
                / f"{model_store.ONNX_BUNDLE_STAGING_PREFIX}operator"
            )
            abandoned.mkdir(parents=True)
            with self.assertRaises(model_store.ModelStoreError):
                model_store.fetch_onnx_bundle(store, source.model_id)
            self.assertTrue(abandoned.is_dir())
            self.assertFalse(
                (store / "bundles" / model_store.ONNX_BUNDLE_STORE_MARKER).exists()
            )

    def test_atomic_bundle_store_claim_resumes_initialization(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary) / "store"
            store.mkdir()
            (store / model_store.ONNX_BUNDLE_STORE_CLAIM).mkdir()
            bundles = model_store._ensure_onnx_bundle_store(store)
            self.assertTrue(
                (bundles / model_store.ONNX_BUNDLE_STORE_MARKER).is_file()
            )
            self.assertEqual(
                list((store / model_store.ONNX_BUNDLE_STORE_CLAIM).iterdir()), []
            )

    def test_onnx_fetch_rejects_symlinked_managed_directories(self) -> None:
        data = b"x"
        source = OnnxModelSource(
            model_id="test",
            model_name="test",
            source_url="https://example.invalid/model.onnx",
            sha256=hashlib.sha256(data).hexdigest(),
            bytes=len(data),
            paddle_model_id="test",
            paddle_inference_json_sha256="1" * 64,
            paddle_inference_yml_sha256="2" * 64,
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            store = root / "store"
            store.mkdir()
            external = root / "external"
            target = external / source.sha256
            target.mkdir(parents=True)
            (target / "inference.onnx").write_bytes(data)
            (store / "objects").symlink_to(external, target_is_directory=True)
            with (
                patch.object(
                    model_store, "load_registered_onnx_source", return_value=source
                ),
                self.assertRaises(model_store.ModelStoreError),
            ):
                model_store.fetch_onnx(store)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            store = root / "store"
            objects = store / "objects"
            objects.mkdir(parents=True)
            external = root / "external" / source.sha256
            external.mkdir(parents=True)
            (external / "inference.onnx").write_bytes(data)
            (objects / source.sha256).symlink_to(external, target_is_directory=True)
            with (
                patch.object(
                    model_store, "load_registered_onnx_source", return_value=source
                ),
                self.assertRaises(model_store.ModelStoreError),
            ):
                model_store.fetch_onnx(store)

    def test_model_fetch_rejects_relative_store_without_creating_it(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            previous = Path.cwd()
            os.chdir(temporary)
            try:
                for fetch in (model_store.fetch, model_store.fetch_onnx):
                    with self.subTest(fetch=fetch.__name__):
                        relative = Path(f"relative-{fetch.__name__}")
                        with self.assertRaises(model_store.ModelStoreError):
                            fetch(relative)
                        self.assertFalse(relative.exists())
            finally:
                os.chdir(previous)

    def test_verified_model_bytes_are_detached_from_source_paths(self) -> None:
        data = b"registered model bytes"
        source = ModelSource(
            model_id="test",
            model_name="test",
            source_url="https://example.invalid/model.tar",
            archive_sha256="1" * 64,
            archive_bytes=1,
            paddleocr_version="test",
            paddlepaddle_version="test",
            files=(
                ModelFile(
                    archive_path="test/inference.json",
                    filename="inference.json",
                    sha256=hashlib.sha256(data).hexdigest(),
                    bytes=len(data),
                ),
            ),
        )
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = directory / "inference.json"
            path.write_bytes(data)
            files = model_store.read_verified_model_files(directory, source)
            path.write_bytes(b"changed")
            self.assertEqual(files, {"inference.json": data})

    def test_ctc_score_sums_blank_repeat_and_direct_alignments(self) -> None:
        probabilities = np.array(
            [
                [0.6, 0.4, 0.0],
                [0.2, 0.7, 0.1],
                [0.5, 0.4, 0.1],
            ],
            dtype=np.float32,
        )
        score = ctc_log_probability(probabilities, [1])
        expected = (
            0.6 * 0.7 * 0.5
            + 0.4 * 0.7 * 0.5
            + 0.6 * 0.7 * 0.4
            + 0.4 * 0.7 * 0.4
            + 0.4 * 0.2 * 0.5
            + 0.6 * 0.2 * 0.4
        )
        self.assertAlmostEqual(math.exp(score), expected, places=6)

    def test_ctc_reference_rejects_impossible_and_nonfinite_values(self) -> None:
        probabilities = np.full((40, 2), 0.5, dtype=np.float32)
        with self.assertRaises(ParityError):
            ctc_log_probability(probabilities, [1] * 21)
        with self.assertRaises(ParityError):
            _canonical_json({"score": math.inf})

    def test_crop_contract_accepts_exact_bytes_and_rejects_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            layout_sha256, definitions = load_layout_contract()
            entries = []
            for index, (field, (filename, roi)) in enumerate(definitions.items()):
                pixels = bytes([index]) * (roi["width"] * roi["height"] * 3)
                header = f'P6\n{roi["width"]} {roi["height"]}\n255\n'.encode()
                data = header + pixels
                (directory / filename).write_bytes(data)
                entries.append(
                    {
                        "field": field,
                        "filename": filename,
                        "roi": roi,
                        "pixel_sha256": hashlib.sha256(pixels).hexdigest(),
                        "file_sha256": hashlib.sha256(data).hexdigest(),
                        "bytes": len(data),
                    }
                )
            manifest = {
                "schema": "scorepeek-private-canonical-result-crops-v1",
                "frame_id": "result-001",
                "frame_extraction_sha256": "1" * 64,
                "canonical_frame_sha256": "2" * 64,
                "normalizer_artifact_sha256": CALIBRATED_NORMALIZER_SHA256,
                "canonical_layout_sha256": layout_sha256,
                "crops": entries,
            }
            manifest_bytes = json.dumps(
                manifest, ensure_ascii=False, separators=(",", ":")
            ).encode() + b"\n"
            (directory / "manifest.json").write_bytes(manifest_bytes)
            digest = hashlib.sha256(manifest_bytes).hexdigest()
            _, crops = load_crops(directory, digest)
            self.assertEqual(len(crops), 6)

            manifest["crops"][0]["roi"] = {
                **manifest["crops"][0]["roi"],
                "x": manifest["crops"][0]["roi"]["x"] + 1,
            }
            changed_manifest = json.dumps(
                manifest, ensure_ascii=False, separators=(",", ":")
            ).encode() + b"\n"
            (directory / "manifest.json").write_bytes(changed_manifest)
            with self.assertRaises(SpikeError):
                load_crops(directory, hashlib.sha256(changed_manifest).hexdigest())

            (directory / "manifest.json").write_bytes(manifest_bytes)

            with (directory / "title.ppm").open("ab") as output:
                output.write(b"x")
            with self.assertRaises(SpikeError):
                load_crops(directory, digest)

    def test_music_select_crop_contract_accepts_selected_and_list_slots(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            layout_sha256, definitions = load_layout_contract("music_select")
            entries = []
            for index, (field, (filename, roi)) in enumerate(definitions.items()):
                pixels = bytes([index]) * (roi["width"] * roi["height"] * 3)
                header = f'P6\n{roi["width"]} {roi["height"]}\n255\n'.encode()
                data = header + pixels
                (directory / filename).write_bytes(data)
                entries.append(
                    {
                        "field": field,
                        "filename": filename,
                        "roi": roi,
                        "pixel_sha256": hashlib.sha256(pixels).hexdigest(),
                        "file_sha256": hashlib.sha256(data).hexdigest(),
                        "bytes": len(data),
                    }
                )
            manifest = {
                "schema": "scorepeek-private-canonical-music-select-crops-v1",
                "frame_id": "music-select-001",
                "frame_extraction_sha256": "1" * 64,
                "canonical_frame_sha256": "2" * 64,
                "normalizer_artifact_sha256": CALIBRATED_NORMALIZER_SHA256,
                "canonical_layout_sha256": layout_sha256,
                "crops": entries,
            }
            manifest_bytes = json.dumps(
                manifest, ensure_ascii=False, separators=(",", ":")
            ).encode() + b"\n"
            (directory / "manifest.json").write_bytes(manifest_bytes)
            digest = hashlib.sha256(manifest_bytes).hexdigest()
            _, crops = load_crops(directory, digest)
            self.assertEqual(len(crops), 21)
            self.assertEqual(crops[0].field, "selected_title")
            self.assertEqual(crops[-1].field, "list_title_19")


if __name__ == "__main__":
    unittest.main()
