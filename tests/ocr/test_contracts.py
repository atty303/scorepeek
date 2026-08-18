from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import scorepeek_ocr.model_store as model_store
from scorepeek_ocr.model_store import load_registered_source
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
