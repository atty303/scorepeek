"""Evaluate an official ONNX recognizer without adapting the song catalog to its vocabulary."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import selectors
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import numpy as np
from rapidfuzz import process
from rapidfuzz.distance import Levenshtein

from scorepeek_ocr.model_store import ModelStoreError, load_registered_onnx_source
from scorepeek_ocr.parity import PREPROCESSOR_ID
from scorepeek_ocr.provisional_labels import (
    ProvisionalLabelError,
    _comparison_key,
    _exact_comparison_key,
    _load_candidates,
)
from scorepeek_ocr.training_artifacts import (
    TrainingArtifactError,
    _prepared_manifest,
    _training_labels,
    _verify_prepared_files,
    prepared_rows,
)
from scorepeek_ocr.training_catalog import (
    CatalogDecisions,
    TrainingCatalogError,
    catalog_candidate_sequences,
    training_truth,
)
from scorepeek_ocr.training_census import SPLITS, TrainingCensusError, summarize_songs
from scorepeek_ocr.training_initializer import (
    MAX_MANIFEST_BYTES,
    TrainingInitializerError,
    _publish,
    _read_regular,
)
from scorepeek_ocr.training_inputs import MAX_INPUT_BYTES

SCHEMA = "scorepeek-private-official-onnx-song-census-v1"
DECODE_SCHEMA = "scorepeek-official-onnx-open-text-batch-v1"
REQUEST_SCHEMA = "scorepeek-private-official-onnx-decode-request-v1"
OBSERVATION_SCHEMA = "scorepeek-private-official-onnx-open-text-observations-v1"
DECODER_TIMEOUT_SECONDS = 10 * 60


class OfficialCensusError(Exception):
    """The official ONNX song-identity census could not be completed."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _validate_decoded_response(raw: Any, row_count: int) -> dict[str, Any]:
    source = load_registered_onnx_source()
    required = {
        "schema",
        "model_id",
        "model_sha256",
        "dictionary_sha256",
        "preprocessor_id",
        "elapsed_ms",
        "decoded_text",
    }
    if (
        not isinstance(raw, dict)
        or set(raw) != required
        or raw["schema"] != DECODE_SCHEMA
        or raw["model_id"] != source.model_id
        or raw["model_sha256"] != source.sha256
        or raw["dictionary_sha256"] != source.paddle_inference_yml_sha256
        or raw["preprocessor_id"] != PREPROCESSOR_ID
        or type(raw["elapsed_ms"]) is not int
        or raw["elapsed_ms"] < 0
        or not isinstance(raw["decoded_text"], list)
        or len(raw["decoded_text"]) != row_count
        or any(not isinstance(value, str) for value in raw["decoded_text"])
    ):
        raise OfficialCensusError("official ONNX decoder result is invalid")
    return raw


def _load_observations(
    path: Path,
    digest: str,
    training_input_sha256: str,
    catalog_candidates_sha256: str,
    labels: list[dict[str, Any]],
) -> dict[str, Any]:
    source = load_registered_onnx_source()
    try:
        raw = json.loads(_read_regular(path, MAX_INPUT_BYTES, digest))
    except json.JSONDecodeError as error:
        raise OfficialCensusError("saved observations are invalid JSON") from error
    required = {
        "schema",
        "training_input_sha256",
        "catalog_candidate_artifact_sha256",
        "model_id",
        "model_sha256",
        "dictionary_sha256",
        "preprocessor_id",
        "rows",
    }
    if (
        not isinstance(raw, dict)
        or set(raw) != required
        or raw["schema"] != OBSERVATION_SCHEMA
        or raw["training_input_sha256"] != training_input_sha256
        or raw["catalog_candidate_artifact_sha256"] != catalog_candidates_sha256
        or raw["model_id"] != source.model_id
        or raw["model_sha256"] != source.sha256
        or raw["dictionary_sha256"] != source.paddle_inference_yml_sha256
        or raw["preprocessor_id"] != PREPROCESSOR_ID
        or not isinstance(raw["rows"], list)
        or len(raw["rows"]) != len(labels)
    ):
        raise OfficialCensusError("saved observation bindings are invalid")
    decoded_text = []
    for row, label in zip(raw["rows"], labels, strict=True):
        if (
            not isinstance(row, dict)
            or set(row) != {"group_id", "crop_file_sha256", "decoded_text"}
            or row["group_id"] != label["group_id"]
            or row["crop_file_sha256"] != label["crop_file_sha256"]
            or not isinstance(row["decoded_text"], str)
        ):
            raise OfficialCensusError("saved observation rows are invalid or reordered")
        decoded_text.append(row["decoded_text"])
    return {
        "schema": DECODE_SCHEMA,
        "model_id": source.model_id,
        "model_sha256": source.sha256,
        "dictionary_sha256": source.paddle_inference_yml_sha256,
        "preprocessor_id": PREPROCESSOR_ID,
        "elapsed_ms": None,
        "decoded_text": decoded_text,
    }


def _queries(text: str) -> tuple[str, ...]:
    return tuple(sorted({text, _exact_comparison_key(text), _comparison_key(text)}))


def _run_bounded(
    command: list[str],
    *,
    timeout: float = DECODER_TIMEOUT_SECONDS,
    stdout_limit: int = MAX_INPUT_BYTES,
    stderr_limit: int = MAX_MANIFEST_BYTES,
    termination_grace: float = 2.0,
) -> tuple[int, bytes, bytes]:
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    streams = [process.stdout, process.stderr]
    if any(stream is None for stream in streams):
        process.kill()
        process.wait()
        raise OfficialCensusError("official ONNX decoder pipes are unavailable")
    buffers = [bytearray(), bytearray()]
    limits = [stdout_limit, stderr_limit]
    selector = selectors.DefaultSelector()
    for index, stream in enumerate(streams):
        assert stream is not None
        os.set_blocking(stream.fileno(), False)
        selector.register(stream, selectors.EVENT_READ, index)
    deadline = time.monotonic() + timeout
    failure: str | None = None
    reap_failure = False
    while process.poll() is None or selector.get_map():
        remaining_time = deadline - time.monotonic()
        if remaining_time <= 0:
            failure = "official ONNX decoder timed out"
            break
        events = selector.select(min(0.05, remaining_time))
        for key, _ in events:
            index = key.data
            try:
                chunk = os.read(key.fd, 64 * 1024)
            except BlockingIOError:
                continue
            if not chunk:
                selector.unregister(key.fileobj)
                continue
            remaining = limits[index] - len(buffers[index])
            if len(chunk) > remaining:
                buffers[index].extend(chunk[: max(0, remaining)])
                failure = "official ONNX decoder output exceeded its bound"
                break
            buffers[index].extend(chunk)
        if failure is not None:
            break
    if failure is not None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        cleanup_deadline = time.monotonic() + termination_grace
        while selector.get_map() and time.monotonic() < cleanup_deadline:
            for key, _ in selector.select(0.05):
                try:
                    chunk = os.read(key.fd, 64 * 1024)
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(key.fileobj)
        if selector.get_map() or process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=max(termination_grace, 0.1))
        except subprocess.TimeoutExpired:
            reap_failure = True
    elif process.poll() is None:
        try:
            process.wait(timeout=max(deadline - time.monotonic(), 0.1))
        except subprocess.TimeoutExpired:
            reap_failure = True
    selector.close()
    for stream in streams:
        assert stream is not None
        stream.close()
    if failure is not None:
        raise OfficialCensusError(failure)
    if reap_failure:
        raise OfficialCensusError("official ONNX decoder could not be reaped")
    return process.returncode, bytes(buffers[0]), bytes(buffers[1])


def _decisions(
    predicted: list[str | None], margins: list[float], expected: list[str]
) -> CatalogDecisions:
    return CatalogDecisions(
        tuple(
            actual is not None and actual == truth
            for actual, truth in zip(predicted, expected, strict=True)
        ),
        tuple(margins),
        tuple(expected),
        tuple(predicted),
    )


def _summarize(
    decisions: CatalogDecisions, labels: list[dict[str, Any]]
) -> tuple[dict[str, int | float | None], list[dict[str, Any]]]:
    summary, unrecognized = summarize_songs(decisions, labels)
    summary["wrong_unique_song_id_decision_count"] = sum(
        predicted is not None and predicted != expected
        for predicted, expected in zip(
            decisions.predicted_song_ids, decisions.expected_song_ids, strict=True
        )
    )
    summary["unknown_or_tied_song_id_decision_count"] = sum(
        predicted is None for predicted in decisions.predicted_song_ids
    )
    return summary, unrecognized


def _exact_decisions(
    decoded: list[str], candidates: list[dict[str, Any]], expected: list[str]
) -> CatalogDecisions:
    exact_songs: dict[str, set[str]] = {}
    folded_songs: dict[str, set[str]] = {}
    for candidate in candidates:
        for variant in candidate["variants"]:
            exact_songs.setdefault(
                _exact_comparison_key(variant["value"]), set()
            ).add(candidate["song_id"])
            folded_songs.setdefault(_comparison_key(variant["value"]), set()).add(
                candidate["song_id"]
            )
    predicted = []
    margins = []
    for text in decoded:
        songs = exact_songs.get(_exact_comparison_key(text), set())
        if not songs:
            songs = folded_songs.get(_comparison_key(text), set())
        predicted.append(next(iter(songs)) if len(songs) == 1 else None)
        margins.append(1.0 if len(songs) == 1 else 0.0)
    return _decisions(predicted, margins, expected)


def _distance_decisions(
    decoded: list[str],
    choices: list[str],
    choice_song_indexes: list[np.ndarray],
    song_ids: list[str],
    expected: list[str],
    *,
    normalized: bool,
) -> CatalogDecisions:
    scorer = Levenshtein.normalized_similarity if normalized else Levenshtein.distance
    dtype = np.float32 if normalized else np.int16
    matrices = [
        process.cdist(
            [transform(text) for text in decoded],
            choices,
            scorer=scorer,
            dtype=dtype,
            workers=-1,
        )
        for transform in (lambda value: value, _exact_comparison_key, _comparison_key)
    ]
    sequence_scores = matrices[0]
    combine = np.maximum if normalized else np.minimum
    for matrix in matrices[1:]:
        combine(sequence_scores, matrix, out=sequence_scores)

    predicted: list[str | None] = []
    margins: list[float] = []
    song_scores = np.empty(len(song_ids), dtype=np.float32)
    aggregate = np.max if normalized else np.min
    for row in sequence_scores:
        for index, columns in enumerate(choice_song_indexes):
            song_scores[index] = aggregate(row[columns])
        order = np.argsort(song_scores)
        if normalized:
            order = order[::-1]
            margin = float(song_scores[order[0]] - song_scores[order[1]])
        else:
            margin = float(song_scores[order[1]] - song_scores[order[0]])
        predicted.append(song_ids[int(order[0])] if margin > 0 else None)
        margins.append(margin)
    return _decisions(predicted, margins, expected)


def _model_record(
    decoded: list[str],
    candidates: list[dict[str, Any]],
    expected: list[str],
    labels: list[dict[str, Any]],
    split_lengths: dict[str, int],
) -> list[dict[str, Any]]:
    sequences_by_song = catalog_candidate_sequences(candidates)
    sequence_songs: dict[str, set[str]] = {}
    for song_id, sequences in sequences_by_song.items():
        for sequence in sequences:
            sequence_songs.setdefault(sequence, set()).add(song_id)
    choices = sorted(sequence_songs)
    song_ids = sorted(sequences_by_song)
    song_indexes = {song_id: index for index, song_id in enumerate(song_ids)}
    choice_columns: list[list[int]] = [[] for _ in song_ids]
    for column, choice in enumerate(choices):
        for song_id in sequence_songs[choice]:
            choice_columns[song_indexes[song_id]].append(column)
    if any(not columns for columns in choice_columns):
        raise OfficialCensusError("catalog song has no searchable title sequence")
    choice_song_indexes = [np.asarray(columns, dtype=np.intp) for columns in choice_columns]

    strategies = {
        "comparison_key_exact": _exact_decisions(decoded, candidates, expected),
        "levenshtein_distance": _distance_decisions(
            decoded, choices, choice_song_indexes, song_ids, expected, normalized=False
        ),
        "levenshtein_normalized_similarity": _distance_decisions(
            decoded, choices, choice_song_indexes, song_ids, expected, normalized=True
        ),
    }
    records = []
    for strategy_id, decisions in strategies.items():
        overall, unrecognized = _summarize(decisions, labels)
        split_records = {}
        offset = 0
        for split in SPLITS:
            stop = offset + split_lengths[split]
            split_summary, split_unrecognized = _summarize(
                CatalogDecisions(
                    decisions.correct[offset:stop],
                    decisions.margins[offset:stop],
                    decisions.expected_song_ids[offset:stop],
                    decisions.predicted_song_ids[offset:stop],
                ),
                labels[offset:stop],
            )
            split_records[split] = {
                "summary": split_summary,
                "unrecognized_song_ids": [row["song_id"] for row in split_unrecognized],
            }
            offset = stop
        records.append(
            {
                "strategy_id": strategy_id,
                "overall": overall,
                "splits": split_records,
                "unrecognized_songs": unrecognized,
            }
        )
    baseline_unrecognized = {
        song["song_id"] for song in records[0]["unrecognized_songs"]
    }
    all_song_ids = set(expected)
    baseline_recognized = all_song_ids - baseline_unrecognized
    for record in records:
        recognized = all_song_ids - {
            song["song_id"] for song in record["unrecognized_songs"]
        }
        record["gained_song_ids_vs_comparison_key_exact"] = sorted(
            recognized - baseline_recognized
        )
        record["lost_song_ids_vs_comparison_key_exact"] = sorted(
            baseline_recognized - recognized
        )
    return records


def run(
    preparation: Path,
    preparation_sha256: str,
    training_input: Path,
    training_input_sha256: str,
    catalog_candidates: Path,
    catalog_candidates_sha256: str,
    model: Path | None,
    dictionary: Path | None,
    observations: Path | None,
    observations_sha256: str | None,
    output: Path,
) -> dict[str, Any]:
    prepared = _prepared_manifest(
        json.loads(
            _read_regular(
                preparation / "manifest.json", MAX_MANIFEST_BYTES, preparation_sha256
            )
        )
    )
    _verify_prepared_files(preparation, prepared)
    if prepared["training_input_sha256"] != training_input_sha256:
        raise OfficialCensusError("training input is not bound to the preparation")
    training_raw = json.loads(
        _read_regular(training_input, MAX_INPUT_BYTES, training_input_sha256)
    )
    candidate_raw = json.loads(
        _read_regular(catalog_candidates, MAX_INPUT_BYTES, catalog_candidates_sha256)
    )
    labels_by_split = _training_labels(training_raw, training_input_sha256)
    candidate_catalog, _, _ = _load_candidates(candidate_raw)
    if candidate_catalog != prepared["catalog_sha256"]:
        raise OfficialCensusError("candidate catalog differs from the preparation")
    if not output.is_absolute() or output.exists() or not output.parent.is_dir():
        raise OfficialCensusError("output must be a new absolute directory")

    rows_by_split = {
        split: prepared_rows(preparation, prepared, split) for split in SPLITS
    }
    expected_by_split = {
        split: training_truth(rows_by_split[split], labels_by_split[split])
        for split in SPLITS
    }
    rows = [row for split in SPLITS for row in rows_by_split[split]]
    labels = [label for split in SPLITS for label in labels_by_split[split]]
    expected = [song_id for split in SPLITS for song_id in expected_by_split[split]]

    reuse = observations is not None or observations_sha256 is not None
    if reuse:
        if observations is None or observations_sha256 is None or model or dictionary:
            raise OfficialCensusError(
                "saved observations require their path and digest without model arguments"
            )
        decoded = _load_observations(
            observations,
            observations_sha256,
            training_input_sha256,
            catalog_candidates_sha256,
            labels,
        )
    else:
        if model is None or dictionary is None:
            raise OfficialCensusError("model and dictionary are required for ONNX inference")
        with tempfile.TemporaryDirectory(
            prefix="scorepeek-official-onnx-census-"
        ) as temporary:
            request = Path(temporary) / "request.json"
            request.write_text(
                json.dumps(
                    {
                        "schema": REQUEST_SCHEMA,
                        "rows": [
                            {"path": path, "file_sha256": digest}
                            for path, _, digest in rows
                        ],
                    },
                    separators=(",", ":"),
                )
                + "\n"
            )
            returncode, stdout, stderr = _run_bounded(
                [
                    "cargo",
                    "run",
                    "--locked",
                    "--quiet",
                    "-p",
                    "scorepeek",
                    "--",
                    "recognition",
                    "title-official-onnx-decode",
                    "--model",
                    str(model),
                    "--dictionary",
                    str(dictionary),
                    "--request",
                    str(request),
                ]
            )
            if returncode != 0:
                raise OfficialCensusError(
                    f"official ONNX decoder failed with exit {returncode}: "
                    f"{stderr.decode(errors='replace').strip()[:8192]}"
                )
            if stderr:
                raise OfficialCensusError(
                    "official ONNX decoder emitted unexpected success diagnostics"
                )
            try:
                decoded = _validate_decoded_response(
                    json.loads(stdout), len(rows)
                )
            except json.JSONDecodeError as error:
                raise OfficialCensusError(
                    "official ONNX decoder returned invalid JSON"
                ) from error

    strategies = _model_record(
        decoded["decoded_text"],
        candidate_raw["candidates"],
        expected,
        labels,
        {split: len(rows_by_split[split]) for split in SPLITS},
    )
    observations = (
        json.dumps(
            {
                "schema": OBSERVATION_SCHEMA,
                "training_input_sha256": training_input_sha256,
                "catalog_candidate_artifact_sha256": catalog_candidates_sha256,
                "model_id": decoded["model_id"],
                "model_sha256": decoded["model_sha256"],
                "dictionary_sha256": decoded["dictionary_sha256"],
                "preprocessor_id": decoded["preprocessor_id"],
                "rows": [
                    {
                        "group_id": label["group_id"],
                        "crop_file_sha256": label["crop_file_sha256"],
                        "decoded_text": text,
                    }
                    for label, text in zip(
                        labels, decoded["decoded_text"], strict=True
                    )
                ],
            },
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        )
        + "\n"
    ).encode()
    record = {
        "schema": SCHEMA,
        "training_preparation_sha256": preparation_sha256,
        "training_input_sha256": training_input_sha256,
        "catalog_candidate_artifact_sha256": catalog_candidates_sha256,
        "comparison_key_id": candidate_raw["comparison_key_id"],
        "catalog_sha256": prepared["catalog_sha256"],
        "provisional": True,
        "accepted_holdout_truth": False,
        "model_id": decoded["model_id"],
        "model_sha256": decoded["model_sha256"],
        "dictionary_sha256": decoded["dictionary_sha256"],
        "preprocessor_id": decoded["preprocessor_id"],
        "inference_elapsed_ms": decoded["elapsed_ms"],
        "reused_observations_sha256": observations_sha256,
        "observation_file": "observations.json",
        "observation_sha256": _sha256(observations),
        "catalog_song_count": len(candidate_raw["candidates"]),
        "strategies": strategies,
    }
    staging = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    try:
        encoded = (
            json.dumps(record, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
            + "\n"
        ).encode()
        (staging / "observations.json").write_bytes(observations)
        (staging / "manifest.json").write_bytes(encoded)
        _publish(staging, output)
    except BaseException:
        if staging.exists():
            shutil.rmtree(staging)
        raise
    return {**record, "artifact_sha256": _sha256(encoded)}


def main() -> None:
    parser = argparse.ArgumentParser()
    for name in ("preparation", "training-input", "catalog-candidates"):
        parser.add_argument(f"--{name}", type=Path, required=True)
        parser.add_argument(f"--{name}-sha256", required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--dictionary", type=Path)
    parser.add_argument("--observations", type=Path)
    parser.add_argument("--observations-sha256")
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = run(
            arguments.preparation,
            arguments.preparation_sha256,
            arguments.training_input,
            arguments.training_input_sha256,
            arguments.catalog_candidates,
            arguments.catalog_candidates_sha256,
            arguments.model,
            arguments.dictionary,
            arguments.observations,
            arguments.observations_sha256,
            arguments.output,
        )
    except (
        OSError,
        ValueError,
        ModelStoreError,
        ProvisionalLabelError,
        TrainingArtifactError,
        TrainingCatalogError,
        TrainingCensusError,
        TrainingInitializerError,
        OfficialCensusError,
    ) as error:
        print(f"scorepeek official ONNX census failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "output": str(arguments.output),
                "artifact_sha256": result["artifact_sha256"],
                "model_id": result["model_id"],
                "strategies": [
                    {"strategy_id": row["strategy_id"], **row["overall"]}
                    for row in result["strategies"]
                ],
            },
            separators=(",", ":"),
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
