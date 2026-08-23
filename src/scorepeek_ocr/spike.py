"""Fail-closed PP-OCRv6 spike over a verified canonical crop artifact."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from scorepeek_ocr.model_store import (
    ModelSource,
    ModelStoreError,
    default_store,
    load_registered_source,
    model_path,
    verify_model,
)

MAX_CROP_MANIFEST_BYTES = 64 * 1024
MAX_LAYOUT_BYTES = 64 * 1024
PROJECT_ROOT = Path(__file__).resolve().parents[2]
CANONICAL_LAYOUT_PATH = PROJECT_ROOT / "crates" / "scorepeek" / "src" / "canonical-layout-v1.json"
CALIBRATED_NORMALIZER_SHA256 = (
    "0441099011fdd09d372d6c9b5e18d6c4f2da2809a653e01f8ccb55756d8658cf"
)


class SpikeError(Exception):
    """The canonical crop or OCR result was invalid."""


@dataclass(frozen=True)
class Crop:
    field: str
    path: Path
    file_sha256: str
    data: bytes


def _valid_sha256(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def _exact_object(value: Any, keys: set[str], context: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise SpikeError(f"invalid {context} fields")
    return value


def _read_regular(path: Path, maximum: int) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise SpikeError(f"not a regular file: {path}")
    size = path.stat().st_size
    if size <= 0 or size > maximum:
        raise SpikeError(f"file size is outside the contract: {path}")
    data = path.read_bytes()
    if len(data) != size:
        raise SpikeError(f"file changed while reading: {path}")
    return data


def load_layout_contract(
    screen: str = "result",
) -> tuple[str, dict[str, tuple[str, dict[str, int]]]]:
    layout_bytes = _read_regular(CANONICAL_LAYOUT_PATH, MAX_LAYOUT_BYTES)
    try:
        raw = json.loads(layout_bytes)
    except json.JSONDecodeError as error:
        raise SpikeError("canonical layout is invalid JSON") from error
    raw = _exact_object(
        raw,
        {
            "schema",
            "canonical_frame_contract_id",
            "width",
            "height",
            "result",
            "music_select",
        },
        "canonical layout",
    )
    if (
        raw["schema"] != "scorepeek-canonical-layout-v1"
        or raw["canonical_frame_contract_id"]
        != "scorepeek-canonical-rgb8-1920x1080-v1"
        or raw["width"] != 1920
        or raw["height"] != 1080
    ):
        raise SpikeError("canonical layout values are invalid")
    result = _exact_object(
        raw["result"],
        {
            "presence",
            "header",
            "upper_panel_edge",
            "lower_panel_edge",
            "title",
            "artist",
            "clear_type",
            "difficulty",
            "level",
            "notes",
            "current_score",
        },
        "canonical result layout",
    )
    result_files = {
        "title": "title.ppm",
        "artist": "artist.ppm",
        "clear_type": "clear-type.ppm",
        "difficulty": "difficulty.ppm",
        "level": "level.ppm",
        "notes": "notes.ppm",
        "current_score": "current-score.ppm",
    }
    _exact_object(
        result["presence"],
        {"warm_pixels_min", "horizontal_edge_pixels_min"},
        "result presence predicate",
    )
    for field in ("header", "upper_panel_edge", "lower_panel_edge"):
        roi = _exact_object(
            result[field], {"x", "y", "width", "height"}, f"{field} ROI"
        )
        if (
            not all(isinstance(roi[key], int) and roi[key] >= 0 for key in roi)
            or roi["width"] == 0
            or roi["height"] == 0
            or roi["x"] + roi["width"] > raw["width"]
            or roi["y"] + roi["height"] > raw["height"]
        ):
            raise SpikeError("canonical result presence ROI is invalid")
    result_expected = {}
    for field, filename in result_files.items():
        roi = _exact_object(
            result[field], {"x", "y", "width", "height"}, f"{field} ROI"
        )
        if (
            not all(isinstance(roi[key], int) and roi[key] >= 0 for key in roi)
            or roi["width"] == 0
            or roi["height"] == 0
            or roi["x"] + roi["width"] > raw["width"]
            or roi["y"] + roi["height"] > raw["height"]
        ):
            raise SpikeError("canonical result ROI is invalid")
        result_expected[field] = (filename, roi)

    music_select = _exact_object(
        raw["music_select"],
        {"presence", "header", "level_column", "selected_title", "list_titles"},
        "canonical music-select layout",
    )
    _exact_object(
        music_select["presence"],
        {"cyan_header_pixels_min", "colored_level_pixels_min"},
        "music-select presence predicate",
    )
    for field in ("header", "level_column", "selected_title"):
        roi = _exact_object(
            music_select[field], {"x", "y", "width", "height"}, f"{field} ROI"
        )
        if (
            not all(isinstance(roi[key], int) and roi[key] >= 0 for key in roi)
            or roi["width"] == 0
            or roi["height"] == 0
            or roi["x"] + roi["width"] > raw["width"]
            or roi["y"] + roi["height"] > raw["height"]
        ):
            raise SpikeError("canonical music-select ROI is invalid")
    repeated = _exact_object(
        music_select["list_titles"],
        {"x", "y", "width", "height", "stride_y", "slots"},
        "music-select list title ROIs",
    )
    if (
        not all(isinstance(repeated[key], int) and repeated[key] >= 0 for key in repeated)
        or repeated["width"] == 0
        or repeated["height"] == 0
        or repeated["stride_y"] == 0
        or repeated["slots"] == 0
        or repeated["x"] + repeated["width"] > raw["width"]
        or repeated["y"]
        + repeated["height"]
        + (repeated["slots"] - 1) * repeated["stride_y"]
        > raw["height"]
    ):
        raise SpikeError("canonical music-select repeated ROI is invalid")
    music_expected = {
        "selected_title": ("selected-title.ppm", music_select["selected_title"])
    }
    for slot in range(repeated["slots"]):
        field = f"list_title_{slot:02}"
        music_expected[field] = (
            f"list-title-{slot:02}.ppm",
            {
                "x": repeated["x"],
                "y": repeated["y"] + slot * repeated["stride_y"],
                "width": repeated["width"],
                "height": repeated["height"],
            },
        )
    expected = {
        "result": result_expected,
        "music_select": music_expected,
    }.get(screen)
    if expected is None:
        raise SpikeError("unsupported crop screen")
    return hashlib.sha256(layout_bytes).hexdigest(), expected


def load_crops(directory: Path, expected_sha256: str) -> tuple[str, list[Crop]]:
    if not directory.is_absolute() or directory.is_symlink() or not directory.is_dir():
        raise SpikeError("crop artifact must be an absolute regular directory")
    if not _valid_sha256(expected_sha256):
        raise SpikeError("crop manifest SHA-256 is invalid")
    manifest_bytes = _read_regular(
        directory / "manifest.json", MAX_CROP_MANIFEST_BYTES
    )
    if hashlib.sha256(manifest_bytes).hexdigest() != expected_sha256:
        raise SpikeError("crop manifest digest mismatch")
    try:
        raw = json.loads(manifest_bytes)
    except json.JSONDecodeError as error:
        raise SpikeError("crop manifest is invalid JSON") from error
    raw = _exact_object(
        raw,
        {
            "schema",
            "frame_id",
            "frame_extraction_sha256",
            "canonical_frame_sha256",
            "normalizer_artifact_sha256",
            "canonical_layout_sha256",
            "crops",
        },
        "crop manifest",
    )
    schema = raw["schema"]
    screen = {
        "scorepeek-private-canonical-result-crops-v1": "result",
        "scorepeek-private-canonical-music-select-crops-v1": "music_select",
    }.get(schema)
    if screen is None:
        raise SpikeError("crop manifest schema is invalid")
    layout_sha256, expected_crops = load_layout_contract(screen)
    if (
        not isinstance(raw["frame_id"], str)
        or not raw["frame_id"]
        or not _valid_sha256(raw["frame_extraction_sha256"])
        or not _valid_sha256(raw["canonical_frame_sha256"])
        or raw["normalizer_artifact_sha256"] != CALIBRATED_NORMALIZER_SHA256
        or raw["canonical_layout_sha256"] != layout_sha256
        or not isinstance(raw["crops"], list)
        or len(raw["crops"]) != len(expected_crops)
    ):
        raise SpikeError("crop manifest values are invalid")

    crops = []
    for item in raw["crops"]:
        item = _exact_object(
            item,
            {
                "field",
                "filename",
                "roi",
                "pixel_sha256",
                "file_sha256",
                "bytes",
            },
            "crop",
        )
        field = item["field"]
        if field not in expected_crops:
            raise SpikeError("crop field or filename is invalid")
        expected_filename, expected_roi = expected_crops[field]
        if item["filename"] != expected_filename:
            raise SpikeError("crop field or filename is invalid")
        roi = _exact_object(item["roi"], {"x", "y", "width", "height"}, "crop ROI")
        if roi != expected_roi:
            raise SpikeError("crop ROI does not match the canonical layout")
        width = roi["width"]
        height = roi["height"]
        header = f"P6\n{width} {height}\n255\n".encode()
        expected_bytes = len(header) + width * height * 3
        if (
            not _valid_sha256(item["pixel_sha256"])
            or not _valid_sha256(item["file_sha256"])
            or item["bytes"] != expected_bytes
        ):
            raise SpikeError("crop digest or size evidence is invalid")
        path = directory / item["filename"]
        data = _read_regular(path, expected_bytes)
        if (
            len(data) != expected_bytes
            or not data.startswith(header)
            or hashlib.sha256(data).hexdigest() != item["file_sha256"]
            or hashlib.sha256(data[len(header) :]).hexdigest()
            != item["pixel_sha256"]
        ):
            raise SpikeError(f"crop bytes mismatch: {field}")
        crops.append(
            Crop(
                field=field,
                path=path,
                file_sha256=item["file_sha256"],
                data=data,
            )
        )
    if {crop.field for crop in crops} != set(expected_crops):
        raise SpikeError("crop set is incomplete or duplicated")
    return raw["frame_extraction_sha256"], crops


def _verify_package_versions(source: ModelSource) -> None:
    installed = {
        "paddleocr": importlib.metadata.version("paddleocr"),
        "paddlepaddle": importlib.metadata.version("paddlepaddle"),
    }
    expected = {
        "paddleocr": source.paddleocr_version,
        "paddlepaddle": source.paddlepaddle_version,
    }
    if installed != expected:
        raise SpikeError("installed OCR package versions do not match the model registration")


def run(
    crop_artifact: Path,
    crop_manifest_sha256: str,
    model_store: Path,
) -> dict[str, Any]:
    frame_extraction_sha256, crops = load_crops(
        crop_artifact, crop_manifest_sha256
    )
    source = load_registered_source()
    model_dir = model_path(model_store, source)
    verify_model(model_dir, source)
    _verify_package_versions(source)

    os.environ["PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK"] = "True"
    from paddleocr import TextRecognition

    started = time.perf_counter()
    predictor = TextRecognition(
        model_name=source.model_name,
        model_dir=str(model_dir),
        device="cpu",
        enable_hpi=False,
    )
    try:
        ordered = sorted(crops, key=lambda crop: crop.field)
        outputs = predictor.predict(
            input=[str(crop.path) for crop in ordered], batch_size=1
        )
    finally:
        predictor.close()
    elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
    if len(outputs) != len(ordered):
        raise SpikeError("OCR result count does not match the crop count")
    results = []
    for crop, output in zip(ordered, outputs, strict=True):
        text = output.get("rec_text")
        score = output.get("rec_score")
        if not isinstance(text, str) or not isinstance(score, float) or not 0 <= score <= 1:
            raise SpikeError(f"invalid OCR result for {crop.field}")
        results.append(
            {
                "field": crop.field,
                "crop_file_sha256": crop.file_sha256,
                "text": text,
                "score": score,
            }
        )
    return {
        "schema": "scorepeek-offline-ocr-spike-v1",
        "frame_extraction_sha256": frame_extraction_sha256,
        "crop_manifest_sha256": crop_manifest_sha256,
        "model_id": source.model_id,
        "model_archive_sha256": source.archive_sha256,
        "device": "cpu",
        "elapsed_ms": elapsed_ms,
        "results": results,
    }


def _validate_output(output: Path) -> None:
    if (
        not output.is_absolute()
        or output.parent.is_symlink()
        or not output.parent.is_dir()
        or output.parent.resolve() != output.parent
        or output.is_symlink()
        or output.exists()
    ):
        raise SpikeError("output path is invalid or already exists")


def _sync_directory(directory: Path) -> None:
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _write_output(output: Path, encoded: str) -> None:
    _validate_output(output)
    descriptor = -1
    staging: Path | None = None
    linked = False
    try:
        descriptor, staging_name = tempfile.mkstemp(
            prefix=".scorepeek-ocr-staging-", dir=output.parent
        )
        staging = Path(staging_name)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="") as stream:
            descriptor = -1
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(staging, output)
        linked = True
        staging.unlink()
        staging = None
        _sync_directory(output.parent)
    except OSError as error:
        if linked and output.exists():
            try:
                output.unlink()
            except OSError:
                pass
        if staging is not None and staging.exists():
            try:
                staging.unlink()
                staging = None
            except OSError:
                pass
        try:
            _sync_directory(output.parent)
        except OSError:
            pass
        raise SpikeError("OCR output could not be published") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--crop-artifact", type=Path, required=True)
    parser.add_argument("--crop-manifest-sha256", required=True)
    parser.add_argument("--model-store", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=None)
    arguments = parser.parse_args()
    try:
        if arguments.output is not None:
            _validate_output(arguments.output)
        store = arguments.model_store or default_store()
        result = run(
            arguments.crop_artifact,
            arguments.crop_manifest_sha256,
            store,
        )
        encoded = json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n"
        if arguments.output is not None:
            _write_output(arguments.output, encoded)
    except (SpikeError, ModelStoreError, OSError) as error:
        print(f"scorepeek OCR spike failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(encoded, end="")


if __name__ == "__main__":
    main()
