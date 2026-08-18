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
from scorepeek_ocr.model_store import (
    ModelFile,
    ModelSource,
    OnnxModelSource,
    load_registered_onnx_source,
    load_registered_source,
)
from scorepeek_ocr.parity import ParityError, _canonical_json, ctc_log_probability
from scorepeek_ocr.spike import (
    CALIBRATED_NORMALIZER_SHA256,
    SpikeError,
    load_crops,
    load_layout_contract,
)


class ContractTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
