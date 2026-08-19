from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import scorepeek_ocr.model_store as model_store
import numpy as np
import unicodedata2 as unicodedata
from scorepeek_ocr.model_store import (
    ModelFile,
    ModelSource,
    OnnxModelSource,
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
from scorepeek_ocr.training_source import (
    TrainingSourceError,
    load_registered_source as load_registered_training_source,
    verify_source as verify_training_source,
)


class ContractTests(unittest.TestCase):
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
