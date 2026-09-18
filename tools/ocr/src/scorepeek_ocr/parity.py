"""Generate a digest-bound Paddle reference for the Rust ONNX parity spike."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import shutil
import sys
import tempfile
import unicodedata
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from scorepeek_ocr.model_store import (
    ModelSource,
    ModelStoreError,
    default_store,
    load_registered_onnx_source,
    load_registered_source,
    model_path,
    read_verified_model_files,
)
from scorepeek_ocr.spike import SpikeError, load_crops

MAX_CANDIDATE_BYTES = 64 * 1024
MAX_CANDIDATES = 1_024
PREPROCESSOR_ID = "paddlex-3.7.0-bgr-rec-resize-3x48x320-v1"
REFERENCE_SCHEMA = "scorepeek-ocr-paddle-parity-reference-v1"


class ParityError(Exception):
    """The parity reference input or Paddle output was invalid."""


@dataclass(frozen=True)
class Candidate:
    song_id: str
    title: str


def _canonical_json(value: Any) -> bytes:
    try:
        encoded = json.dumps(
            value, ensure_ascii=False, separators=(",", ":"), allow_nan=False
        )
    except ValueError as error:
        raise ParityError("parity JSON contains a non-finite number") from error
    return encoded.encode() + b"\n"


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _read_candidates(path: Path) -> tuple[str, list[Candidate]]:
    if path.is_symlink() or not path.is_file():
        raise ParityError("candidate input is not a regular file")
    data = path.read_bytes()
    if not 0 < len(data) <= MAX_CANDIDATE_BYTES:
        raise ParityError("candidate input is outside the size contract")
    try:
        raw = json.loads(data)
    except json.JSONDecodeError as error:
        raise ParityError("candidate input is invalid JSON") from error
    if (
        not isinstance(raw, dict)
        or set(raw) != {"schema", "candidates"}
        or raw["schema"] != "scorepeek-ocr-parity-candidates-v1"
        or not isinstance(raw["candidates"], list)
        or not 2 <= len(raw["candidates"]) <= MAX_CANDIDATES
        or _canonical_json(raw) != data
    ):
        raise ParityError("candidate input values are invalid")
    candidates = []
    ids = set()
    for item in raw["candidates"]:
        if not isinstance(item, dict) or set(item) != {"song_id", "title"}:
            raise ParityError("candidate fields are invalid")
        song_id = item["song_id"]
        title = item["title"]
        try:
            parsed_id = uuid.UUID(song_id)
        except (AttributeError, ValueError) as error:
            raise ParityError("candidate song ID is invalid") from error
        if (
            str(parsed_id) != song_id
            or song_id in ids
            or not isinstance(title, str)
            or not 0 < len(title) <= 256
            or any(unicodedata.category(character).startswith("C") for character in title)
        ):
            raise ParityError("candidate values are invalid")
        ids.add(song_id)
        candidates.append(Candidate(song_id=song_id, title=title))
    return _sha256(data), candidates


def _encode_title(title: str, characters: list[str]) -> list[int]:
    indexes: dict[str, int] = {}
    duplicates = set()
    for index, character in enumerate(characters):
        if character in indexes:
            duplicates.add(character)
        else:
            indexes[character] = index
    tokens = []
    for character in title:
        if character in duplicates or character not in indexes:
            raise ParityError("candidate title is not uniquely encodable by the model dictionary")
        token = indexes[character]
        if token == 0:
            raise ParityError("candidate title contains the CTC blank token")
        tokens.append(token)
    return tokens


def _logsumexp(values: tuple[float, ...]) -> float:
    maximum = max(values)
    if maximum == -math.inf:
        return maximum
    return maximum + math.log(sum(math.exp(value - maximum) for value in values))


def ctc_log_probability(probabilities: np.ndarray, tokens: list[int]) -> float:
    if probabilities.ndim != 2 or not tokens:
        raise ParityError("CTC score input shape is invalid")
    classes = probabilities.shape[1]
    if any(token <= 0 or token >= classes for token in tokens):
        raise ParityError("CTC candidate token is outside the model output")
    required_timesteps = len(tokens) + sum(
        left == right for left, right in zip(tokens, tokens[1:], strict=False)
    )
    if required_timesteps > probabilities.shape[0]:
        raise ParityError("CTC candidate cannot align within the model timesteps")
    labels = [0]
    for token in tokens:
        labels.extend((token, 0))
    previous = [-math.inf] * len(labels)
    previous[0] = math.log(float(probabilities[0, 0]))
    if len(labels) > 1:
        previous[1] = math.log(float(probabilities[0, labels[1]]))
    for timestep in range(1, probabilities.shape[0]):
        current = [-math.inf] * len(labels)
        for state, token in enumerate(labels):
            sources = [previous[state]]
            if state > 0:
                sources.append(previous[state - 1])
            if state > 1 and token != 0 and token != labels[state - 2]:
                sources.append(previous[state - 2])
            probability = float(probabilities[timestep, token])
            if not math.isfinite(probability) or probability <= 0:
                raise ParityError("Paddle CTC probability is invalid")
            current[state] = _logsumexp(tuple(sources)) + math.log(probability)
        previous = current
    score = _logsumexp((previous[-1], previous[-2]))
    if not math.isfinite(score):
        raise ParityError("CTC candidate score is not finite")
    return score


def _argmax_tokens(probabilities: np.ndarray) -> tuple[list[int], list[int]]:
    raw = probabilities.argmax(axis=1).tolist()
    collapsed = []
    previous = None
    for token in raw:
        if token != 0 and token != previous:
            collapsed.append(token)
        previous = token
    return raw, collapsed


def _tensor_bytes(tensor: np.ndarray) -> bytes:
    if tensor.dtype != np.float32 or not np.isfinite(tensor).all():
        raise ParityError("Paddle tensor is not finite float32")
    return tensor.astype("<f4", copy=False).tobytes(order="C")


def _decode_bgr_image(data: bytes) -> np.ndarray:
    import cv2

    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None or image.ndim != 3 or image.shape[2] != 3:
        raise ParityError("verified title crop cannot be decoded as BGR")
    return image


def _write_model_snapshot(
    source_directory: Path, source: ModelSource, snapshot_directory: Path
) -> None:
    files = read_verified_model_files(source_directory, source)
    snapshot_directory.mkdir()
    for filename, data in files.items():
        (snapshot_directory / filename).write_bytes(data)


def generate(
    crop_artifact: Path,
    crop_manifest_sha256: str,
    candidate_file: Path,
    model_store: Path,
    output: Path,
) -> dict[str, Any]:
    frame_extraction_sha256, crops = load_crops(crop_artifact, crop_manifest_sha256)
    title_crop = next((crop for crop in crops if crop.field == "title"), None)
    if title_crop is None:
        raise ParityError("verified crop artifact has no title crop")
    candidate_sha256, candidates = _read_candidates(candidate_file)

    paddle_source = load_registered_source()
    onnx_source = load_registered_onnx_source()
    paddle_dir = model_path(model_store, paddle_source)
    installed = {
        "paddleocr": importlib.metadata.version("paddleocr"),
        "paddlepaddle": importlib.metadata.version("paddlepaddle"),
    }
    if installed != {
        "paddleocr": paddle_source.paddleocr_version,
        "paddlepaddle": paddle_source.paddlepaddle_version,
    }:
        raise ParityError("installed OCR packages do not match the model registration")

    os.environ["PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK"] = "True"
    from paddleocr import TextRecognition

    with tempfile.TemporaryDirectory(prefix="scorepeek-ocr-model-") as temporary:
        snapshot_dir = Path(temporary) / "model"
        _write_model_snapshot(paddle_dir, paddle_source, snapshot_dir)
        predictor = TextRecognition(
            model_name=paddle_source.model_name,
            model_dir=str(snapshot_dir),
            device="cpu",
            enable_hpi=False,
        )
        try:
            runner = predictor.paddlex_predictor
            images = runner.pre_tfs["Read"](imgs=[_decode_bgr_image(title_crop.data)])
            resized = runner.pre_tfs["ReisizeNorm"](imgs=images)
            input_tensor = runner.pre_tfs["ToBatch"](imgs=resized)[0]
            paddle_output = np.asarray(runner.runner(x=[input_tensor])[0])
            characters = runner.post_op.character
        finally:
            predictor.close()

    if input_tensor.shape != (1, 3, 48, 320) or paddle_output.shape != (1, 40, 18710):
        raise ParityError("Paddle tensor shape differs from the registered contract")
    if (
        not isinstance(characters, list)
        or len(characters) != paddle_output.shape[2]
        or characters[0] != "blank"
        or characters[-1] != " "
    ):
        raise ParityError("Paddle dictionary differs from the registered contract")
    probabilities = paddle_output[0]
    sums = probabilities.sum(axis=1)
    if not np.isfinite(probabilities).all() or not np.all(np.abs(sums - 1.0) <= 2e-5):
        raise ParityError("Paddle graph output is not a probability tensor")

    ranked = []
    for candidate in candidates:
        tokens = _encode_title(candidate.title, characters)
        ranked.append(
            {
                "song_id": candidate.song_id,
                "title": candidate.title,
                "tokens": tokens,
                "paddle_log_probability": ctc_log_probability(probabilities, tokens),
            }
        )
    ranked.sort(key=lambda item: (-item["paddle_log_probability"], item["song_id"]))
    raw_tokens, collapsed_tokens = _argmax_tokens(probabilities)
    input_bytes = _tensor_bytes(input_tensor)
    output_bytes = _tensor_bytes(paddle_output)

    output.mkdir()
    try:
        (output / "input.f32le").write_bytes(input_bytes)
        (output / "paddle-output.f32le").write_bytes(output_bytes)
        manifest = {
            "schema": REFERENCE_SCHEMA,
            "frame_extraction_sha256": frame_extraction_sha256,
            "crop_manifest_sha256": crop_manifest_sha256,
            "title_crop_file_sha256": title_crop.file_sha256,
            "candidate_source_sha256": candidate_sha256,
            "paddle_model_id": paddle_source.model_id,
            "paddle_model_archive_sha256": paddle_source.archive_sha256,
            "onnx_model_id": onnx_source.model_id,
            "onnx_model_sha256": onnx_source.sha256,
            "paddle_inference_json_sha256": onnx_source.paddle_inference_json_sha256,
            "paddle_inference_yml_sha256": onnx_source.paddle_inference_yml_sha256,
            "preprocessor_id": PREPROCESSOR_ID,
            "input": {
                "filename": "input.f32le",
                "sha256": _sha256(input_bytes),
                "bytes": len(input_bytes),
                "shape": list(input_tensor.shape),
            },
            "paddle_output": {
                "filename": "paddle-output.f32le",
                "sha256": _sha256(output_bytes),
                "bytes": len(output_bytes),
                "shape": list(paddle_output.shape),
            },
            "ctc_blank_token": 0,
            "argmax_token_order": raw_tokens,
            "collapsed_token_order": collapsed_tokens,
            "candidate_ranking": ranked,
        }
        manifest_bytes = _canonical_json(manifest)
        (output / "manifest.json").write_bytes(manifest_bytes)
    except BaseException:
        shutil.rmtree(output, ignore_errors=True)
        raise
    return {
        "schema": "scorepeek-ocr-paddle-parity-reference-summary-v1",
        "output": str(output),
        "manifest_sha256": _sha256(manifest_bytes),
        "top_candidate_song_id": ranked[0]["song_id"],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--crop-artifact", type=Path, required=True)
    parser.add_argument("--crop-manifest-sha256", required=True)
    parser.add_argument("--candidates", type=Path, required=True)
    parser.add_argument("--model-store", type=Path, default=None)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = generate(
            arguments.crop_artifact,
            arguments.crop_manifest_sha256,
            arguments.candidates,
            arguments.model_store or default_store(),
            arguments.output,
        )
    except (ParityError, SpikeError, ModelStoreError, OSError) as error:
        print(f"scorepeek OCR parity reference failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
