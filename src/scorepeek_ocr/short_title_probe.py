"""Reproduce bounded OCR signal observations for private one-character titles."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import shutil
import sqlite3
import stat
import sys
import tempfile
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from scorepeek_ocr.model_store import (
    ModelStoreError,
    default_store,
    load_registered_source,
    model_path,
    read_verified_model_files,
)
from scorepeek_ocr.parity import ParityError, ctc_log_probability
from scorepeek_ocr.provisional_labels import _valid_sha256
from scorepeek_ocr.training_artifacts import (
    MAX_CROP_BYTES,
    TrainingArtifactError,
    _crop_map,
    _read as _read_artifact,
    _training_labels,
)
from scorepeek_ocr.training_initializer import _publish
from scorepeek_ocr.training_inputs import MAX_INPUT_BYTES, TrainingInputError, _read

SCHEMA = "scorepeek-private-short-title-probe-v1"
ORIGINAL_VIEW = "scorepeek-short-title-original-v1"
TIGHT_VIEW = "scorepeek-short-title-gray80-bbox-x12-y1-v1"
HORIZONTAL_VIEW = "scorepeek-short-title-gray80-bbox-x4-full-y-v1"
VIEW_IDS = (ORIGINAL_VIEW, TIGHT_VIEW, HORIZONTAL_VIEW)
CATALOG_MAX_BYTES = 128 * 1024 * 1024
ALIAS_TITLE = "〆"
ALIAS_SEQUENCE = "x"
ALIAS_LOG_SCORE_BIAS = 0.25


class ShortTitleProbeError(Exception):
    """The private short-title observation could not be reproduced."""


@dataclass(frozen=True)
class Label:
    group_id: str
    song_id: str
    title: str
    crop_file_sha256: str
    crop_pixel_sha256: str
    path: Path


@dataclass(frozen=True)
class Catalog:
    variants_by_song: dict[str, tuple[str, ...]]
    title_songs: dict[str, frozenset[str]]


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _canonical_json(value: Any) -> bytes:
    try:
        return (
            json.dumps(
                value,
                ensure_ascii=False,
                separators=(",", ":"),
                allow_nan=False,
            )
            + "\n"
        ).encode()
    except ValueError as error:
        raise ShortTitleProbeError("probe output contains a non-finite value") from error


def _snapshot_verified_file(
    path: Path, expected: str, maximum: int, snapshot: Path
) -> int:
    if (
        not path.is_absolute()
        or not _valid_sha256(expected)
        or not snapshot.is_absolute()
    ):
        raise ShortTitleProbeError(f"digest-bound input is invalid: {path}")
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= maximum:
            raise ShortTitleProbeError(f"digest-bound input size is invalid: {path}")
        digest = hashlib.sha256()
        total = 0
        with snapshot.open("xb") as destination:
            while chunk := source.read(1024 * 1024):
                total += len(chunk)
                if total > maximum:
                    raise ShortTitleProbeError(
                        f"digest-bound input size is invalid: {path}"
                    )
                digest.update(chunk)
                destination.write(chunk)
        after = os.fstat(source.fileno())
    identity = lambda value: (
        value.st_dev,
        value.st_ino,
        value.st_size,
        value.st_mtime_ns,
    )
    if (
        total != before.st_size
        or identity(before) != identity(after)
        or digest.hexdigest() != expected
    ):
        raise ShortTitleProbeError(f"digest-bound input mismatched: {path}")
    return total


def _load_labels(
    training_input: Path,
    training_input_sha256: str,
    crop_map_path: Path,
    crop_map_sha256: str,
) -> list[Label]:
    training_raw = _read(training_input, training_input_sha256)
    crop_raw = _read(crop_map_path, crop_map_sha256)
    labels_by_split = _training_labels(training_raw, training_input_sha256)
    crops = _crop_map(crop_raw, training_input_sha256)
    labels = []
    for split in ("train", "validation", "evaluation"):
        for row in labels_by_split[split]:
            crop = crops.get(row["group_id"])
            if (
                crop is None
                or crop["file_sha256"] != row["crop_file_sha256"]
                or crop["pixel_sha256"] != row["crop_pixel_sha256"]
            ):
                raise ShortTitleProbeError("training label and crop map differ")
            labels.append(
                Label(
                    group_id=row["group_id"],
                    song_id=row["song_id"],
                    title=row["title"],
                    crop_file_sha256=row["crop_file_sha256"],
                    crop_pixel_sha256=row["crop_pixel_sha256"],
                    path=crop["path"],
                )
            )
    if len(labels) != len(crops):
        raise ShortTitleProbeError("training labels do not cover the crop map")
    return labels


def _load_catalog(path: Path, digest: str) -> Catalog:
    with tempfile.TemporaryDirectory(prefix="scorepeek-short-title-catalog-") as temporary:
        snapshot = Path(temporary) / "catalog.sqlite3"
        _snapshot_verified_file(path, digest, CATALOG_MAX_BYTES, snapshot)
        connection = sqlite3.connect(
            f"{snapshot.as_uri()}?mode=ro&immutable=1", uri=True
        )
        try:
            song_ids = {
                row[0]
                for row in connection.execute("SELECT song_id FROM songs ORDER BY song_id")
            }
            variants: dict[str, set[str]] = defaultdict(set)
            title_songs: dict[str, set[str]] = defaultdict(set)
            for song_id, value in connection.execute(
                "SELECT DISTINCT song_id, value FROM title_variants "
                "WHERE variant_kind != 'search_term' ORDER BY song_id, value"
            ):
                variants[song_id].add(value)
                title_songs[value].add(song_id)
        except sqlite3.DatabaseError as error:
            raise ShortTitleProbeError(f"catalog query failed: {error}") from error
        finally:
            connection.close()
    if song_ids != set(variants) or not song_ids:
        raise ShortTitleProbeError("catalog title coverage is incomplete")
    return Catalog(
        variants_by_song={
            song_id: tuple(sorted(values)) for song_id, values in variants.items()
        },
        title_songs={title: frozenset(ids) for title, ids in title_songs.items()},
    )


def _decode_image(label: Label) -> np.ndarray:
    data = _read_artifact(label.path, label.crop_file_sha256, MAX_CROP_BYTES)
    first, separator, remainder = data.partition(b"\n")
    dimensions, separator2, remainder = remainder.partition(b"\n")
    maximum, separator3, pixels = remainder.partition(b"\n")
    if (
        first != b"P6"
        or not separator
        or not separator2
        or not separator3
        or maximum != b"255"
        or len(dimensions.split()) != 2
        or not all(part.isdigit() and int(part) > 0 for part in dimensions.split())
    ):
        raise ShortTitleProbeError("crop is not a strict P6 PPM")
    width, height = (int(part) for part in dimensions.split())
    if len(pixels) != width * height * 3 or _sha256(pixels) != label.crop_pixel_sha256:
        raise ShortTitleProbeError("crop pixel evidence mismatched")
    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None or image.ndim != 3 or image.shape[2] != 3:
        raise ShortTitleProbeError(f"crop could not be decoded: {label.group_id}")
    return image


def _foreground(image: np.ndarray) -> tuple[int, int, int, int]:
    mask = (cv2.cvtColor(image, cv2.COLOR_BGR2GRAY) > 80).astype(np.uint8)
    points = cv2.findNonZero(mask)
    if points is None:
        raise ShortTitleProbeError("foreground threshold produced no pixels")
    return tuple(int(value) for value in cv2.boundingRect(points))


def _view(image: np.ndarray, box: tuple[int, int, int, int], view_id: str) -> np.ndarray:
    if view_id == ORIGINAL_VIEW:
        return image
    x, y, width, height = box
    if view_id == TIGHT_VIEW:
        x_margin, y_margin = 12, 1
        y0, y1 = max(0, y - y_margin), min(image.shape[0], y + height + y_margin)
    elif view_id == HORIZONTAL_VIEW:
        x_margin = 4
        y0, y1 = 0, image.shape[0]
    else:
        raise ShortTitleProbeError("short-title view is not registered")
    x0, x1 = max(0, x - x_margin), min(image.shape[1], x + width + x_margin)
    return image[y0:y1, x0:x1]


def _single_token_scores(probabilities: np.ndarray) -> np.ndarray:
    if probabilities.ndim != 2 or probabilities.shape[1] < 2:
        raise ShortTitleProbeError("single-token probability shape is invalid")
    blank = probabilities[:, 0].astype(np.float64)
    characters = probabilities[:, 1:].astype(np.float64)
    before = np.ones(characters.shape[1], dtype=np.float64)
    token = np.zeros(characters.shape[1], dtype=np.float64)
    after = np.zeros(characters.shape[1], dtype=np.float64)
    for timestep in range(probabilities.shape[0]):
        old_before, old_token, old_after = before, token, after
        before = old_before * blank[timestep]
        token = (old_before + old_token) * characters[timestep]
        after = (old_token + old_after) * blank[timestep]
    return token + after


def _token_indexes(characters: list[str]) -> dict[str, int]:
    indexes: dict[str, int] = {}
    duplicates: set[str] = set()
    for index, character in enumerate(characters):
        if index == 0:
            continue
        if character in indexes:
            duplicates.add(character)
        else:
            indexes[character] = index
    for character in duplicates:
        indexes.pop(character)
    return indexes


def _argmax_text(probabilities: np.ndarray, characters: list[str]) -> str:
    result = []
    previous = None
    for token in probabilities.argmax(axis=1).tolist():
        if token and token != previous:
            result.append(characters[token])
        previous = token
    return "".join(result)


def _encode_catalog(
    catalog: Catalog, indexes: dict[str, int]
) -> tuple[dict[str, tuple[tuple[int, ...], ...]], int, int]:
    encoded: dict[str, tuple[tuple[int, ...], ...]] = {}
    unsupported_variants = 0
    for song_id, variants in catalog.variants_by_song.items():
        sequences = []
        for value in variants:
            try:
                sequences.append(tuple(indexes[character] for character in value))
            except KeyError:
                unsupported_variants += 1
        if sequences:
            encoded[song_id] = tuple(sequences)
    return encoded, unsupported_variants, len(catalog.variants_by_song) - len(encoded)


def _required_timesteps(tokens: tuple[int, ...]) -> int:
    return len(tokens) + sum(
        left == right for left, right in zip(tokens, tokens[1:], strict=False)
    )


def _catalog_ranking(
    probabilities: np.ndarray,
    encoded: dict[str, tuple[tuple[int, ...], ...]],
    truth_song_id: str,
    catalog: Catalog,
    alias_song_id: str | None,
    alias_tokens: list[int] | None,
) -> dict[str, Any]:
    scores: dict[str, float] = {}
    for song_id, sequences in encoded.items():
        alignable = [
            sequence
            for sequence in sequences
            if sequence and _required_timesteps(sequence) <= probabilities.shape[0]
        ]
        if alignable:
            scores[song_id] = max(
                ctc_log_probability(probabilities, list(sequence))
                for sequence in alignable
            )
    if truth_song_id not in scores:
        raise ShortTitleProbeError("target song is not scoreable in the catalog")

    base_scores = scores.copy()
    alias_score = None
    if alias_song_id is not None and alias_tokens is not None:
        alias_score = ctc_log_probability(probabilities, alias_tokens)
        scores[alias_song_id] = max(
            scores.get(alias_song_id, -math.inf), alias_score + ALIAS_LOG_SCORE_BIAS
        )

    def result(ranked_scores: dict[str, float]) -> dict[str, Any]:
        ranked = sorted(ranked_scores, key=lambda item: (-ranked_scores[item], item))
        runner_up = max(
            score for song_id, score in ranked_scores.items() if song_id != truth_song_id
        )
        return {
            "scoreable_song_count": len(ranked_scores),
            "unscoreable_song_count": len(catalog.variants_by_song)
            - len(ranked_scores),
            "truth_rank": ranked.index(truth_song_id) + 1,
            "truth_minus_runner_up": float(ranked_scores[truth_song_id] - runner_up),
            "top_song_ids": ranked[:5],
            "top_titles": [catalog.variants_by_song[song_id][0] for song_id in ranked[:5]],
        }

    return {
        "without_alias": result(base_scores),
        "with_alias": result(scores) if alias_score is not None else None,
        "alias_log_probability": alias_score,
    }


def _inference(
    runner: Any, image: np.ndarray, characters: list[str]
) -> tuple[np.ndarray, list[int]]:
    images = runner.pre_tfs["Read"](imgs=[image])
    resized = runner.pre_tfs["ReisizeNorm"](imgs=images)
    tensor = runner.pre_tfs["ToBatch"](imgs=resized)[0]
    output = np.asarray(runner.runner(x=[tensor])[0])
    if (
        output.ndim != 3
        or output.shape[0] != 1
        or output.shape[2] != len(characters)
        or not np.isfinite(output).all()
        or np.max(np.abs(output.sum(axis=2) - 1.0)) > 1e-3
    ):
        raise ShortTitleProbeError("registered Paddle output is invalid")
    return output[0], [int(value) for value in tensor.shape]


def run(
    training_input: Path,
    training_input_sha256: str,
    crop_map_path: Path,
    crop_map_sha256: str,
    catalog_path: Path,
    catalog_sha256: str,
    tight_group_ids: list[str],
    horizontal_group_ids: list[str],
    store: Path,
    output: Path,
) -> dict[str, Any]:
    labels = _load_labels(
        training_input,
        training_input_sha256,
        crop_map_path,
        crop_map_sha256,
    )
    by_group = {label.group_id: label for label in labels}
    target_group_ids = tight_group_ids + horizontal_group_ids
    if (
        not tight_group_ids
        or not horizontal_group_ids
        or len(set(target_group_ids)) != len(target_group_ids)
        or any(group_id not in by_group for group_id in target_group_ids)
        or any(len(by_group[group_id].title) != 1 for group_id in target_group_ids)
    ):
        raise ShortTitleProbeError("explicit target group IDs are invalid")
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise ShortTitleProbeError("output must be a new absolute directory")

    catalog = _load_catalog(catalog_path, catalog_sha256)
    one_character = sorted(
        (label for label in labels if len(label.title) == 1), key=lambda item: item.group_id
    )
    images: dict[str, np.ndarray] = {}
    boxes: dict[str, tuple[int, int, int, int]] = {}
    for label in one_character:
        image = _decode_image(label)
        box = _foreground(image)
        images[label.group_id] = image
        boxes[label.group_id] = box

    paddle_source = load_registered_source()
    installed = {
        "paddleocr": importlib.metadata.version("paddleocr"),
        "paddlepaddle": importlib.metadata.version("paddlepaddle"),
    }
    if installed != {
        "paddleocr": paddle_source.paddleocr_version,
        "paddlepaddle": paddle_source.paddlepaddle_version,
    }:
        raise ShortTitleProbeError("installed OCR packages differ from registration")
    files = read_verified_model_files(model_path(store, paddle_source), paddle_source)

    os.environ["PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK"] = "True"
    from paddleocr import TextRecognition

    with tempfile.TemporaryDirectory(prefix="scorepeek-short-title-model-") as temporary:
        snapshot = Path(temporary) / "model"
        snapshot.mkdir()
        for filename, data in files.items():
            (snapshot / filename).write_bytes(data)
        predictor = TextRecognition(
            model_name=paddle_source.model_name,
            model_dir=str(snapshot),
            device="cpu",
            enable_hpi=False,
        )
        try:
            runner = predictor.paddlex_predictor
            characters = runner.post_op.character
            if (
                not isinstance(characters, list)
                or len(characters) != 18_710
                or characters[0] != "blank"
            ):
                raise ShortTitleProbeError("registered Paddle dictionary is invalid")
            indexes = _token_indexes(characters)
            encoded, unsupported_variants, unencodable_songs = _encode_catalog(
                catalog, indexes
            )
            alias_songs = catalog.title_songs.get(ALIAS_TITLE, frozenset())
            if len(alias_songs) != 1 or ALIAS_SEQUENCE not in indexes:
                raise ShortTitleProbeError("diagnostic alias is not uniquely bound")
            alias_song_id = next(iter(alias_songs))
            alias_tokens = [indexes[ALIAS_SEQUENCE]]

            observation_rows = []
            target_views = {
                **{group_id: TIGHT_VIEW for group_id in tight_group_ids},
                **{group_id: HORIZONTAL_VIEW for group_id in horizontal_group_ids},
            }
            for label in one_character:
                view_records = {}
                for view_id in VIEW_IDS:
                    probabilities, input_shape = _inference(
                        runner,
                        _view(images[label.group_id], boxes[label.group_id], view_id),
                        characters,
                    )
                    single_scores = _single_token_scores(probabilities)
                    order = np.argsort(-single_scores, kind="stable")
                    truth_token = indexes.get(label.title)
                    record: dict[str, Any] = {
                        "input_shape": input_shape,
                        "output_timesteps": int(probabilities.shape[0]),
                        "argmax_text": _argmax_text(probabilities, characters),
                        "top_single_tokens": [
                            {
                                "token": characters[int(index) + 1],
                                "probability": float(single_scores[index]),
                            }
                            for index in order[:5]
                        ],
                        "truth_single_token_rank": None,
                    }
                    if truth_token is not None:
                        record["truth_single_token_rank"] = (
                            int(np.flatnonzero(order == truth_token - 1)[0]) + 1
                        )
                    if label.group_id in target_views and target_views[label.group_id] == view_id:
                        use_alias = view_id == HORIZONTAL_VIEW
                        record["catalog_ranking"] = _catalog_ranking(
                            probabilities,
                            encoded,
                            label.song_id,
                            catalog,
                            alias_song_id if use_alias else None,
                            alias_tokens if use_alias else None,
                        )
                    view_records[view_id] = record
                x, y, width, height = boxes[label.group_id]
                image = images[label.group_id]
                observation_rows.append(
                    {
                        "group_id": label.group_id,
                        "song_id": label.song_id,
                        "title": label.title,
                        "crop_file_sha256": label.crop_file_sha256,
                        "crop_pixel_sha256": label.crop_pixel_sha256,
                        "foreground_box": [x, y, width, height],
                        "foreground_width_ratio": width / image.shape[1],
                        "foreground_height_ratio": height / image.shape[0],
                        "touches_horizontal_edge": x == 0 or x + width == image.shape[1],
                        "views": view_records,
                    }
                )
        finally:
            predictor.close()

    record = {
        "schema": SCHEMA,
        "training_input_sha256": training_input_sha256,
        "crop_map_sha256": crop_map_sha256,
        "catalog_sha256": catalog_sha256,
        "paddle_model_id": paddle_source.model_id,
        "paddle_model_archive_sha256": paddle_source.archive_sha256,
        "provisional": True,
        "accepted_holdout_truth": False,
        "parameters": {
            "views": list(VIEW_IDS),
            "foreground_grayscale_threshold": 80,
            "tight_margin": {"x": 12, "y": 1},
            "horizontal_margin": {"x": 4, "full_height": True},
            "alias_title": ALIAS_TITLE,
            "alias_sequence": ALIAS_SEQUENCE,
            "alias_log_score_bias": ALIAS_LOG_SCORE_BIAS,
            "tight_group_ids": tight_group_ids,
            "horizontal_group_ids": horizontal_group_ids,
        },
        "catalog": {
            "song_count": len(catalog.variants_by_song),
            "dictionary_encodable_song_count": len(encoded),
            "dictionary_unencodable_song_count": unencodable_songs,
            "unsupported_variant_count": unsupported_variants,
        },
        "one_character_observations": {
            "crop_count": len(one_character),
            "song_count": len({label.song_id for label in one_character}),
            "titles": sorted({label.title for label in one_character}),
            "rows": observation_rows,
        },
    }
    encoded_record = _canonical_json(record)
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        (staging / "manifest.json").write_bytes(encoded_record)
        _publish(staging, output)
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise
    return {**record, "artifact_sha256": _sha256(encoded_record)}


def main() -> None:
    parser = argparse.ArgumentParser()
    for name in ("training-input", "crop-map", "catalog"):
        parser.add_argument(f"--{name}", type=Path, required=True)
        parser.add_argument(f"--{name}-sha256", required=True)
    parser.add_argument("--tight-group-id", action="append", default=[])
    parser.add_argument("--horizontal-group-id", action="append", default=[])
    parser.add_argument("--model-store", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.training_input,
            arguments.training_input_sha256,
            arguments.crop_map,
            arguments.crop_map_sha256,
            arguments.catalog,
            arguments.catalog_sha256,
            arguments.tight_group_id,
            arguments.horizontal_group_id,
            arguments.model_store or default_store(),
            arguments.output,
        )
    except (
        OSError,
        sqlite3.DatabaseError,
        ModelStoreError,
        ParityError,
        TrainingArtifactError,
        TrainingInputError,
        ShortTitleProbeError,
    ) as error:
        print(f"scorepeek short-title probe failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "output": str(arguments.output),
                "artifact_sha256": result["artifact_sha256"],
                "one_character_crop_count": result["one_character_observations"][
                    "crop_count"
                ],
            },
            ensure_ascii=False,
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
