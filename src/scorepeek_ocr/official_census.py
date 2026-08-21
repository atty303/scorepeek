"""Evaluate an official ONNX recognizer without adapting the song catalog to its vocabulary."""

from __future__ import annotations

import argparse
import ctypes
import fcntl
import hashlib
import json
import os
import selectors
import signal
import stat
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
from rapidfuzz import process
from rapidfuzz.distance import Levenshtein

from scorepeek_ocr.model_store import (
    ModelStoreError,
    load_registered_onnx_bundle,
    load_registered_onnx_source,
)
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
DYNAMIC_DECODE_SCHEMA = "scorepeek-official-onnx-dynamic-open-text-batch-v1"
REQUEST_SCHEMA = "scorepeek-private-official-onnx-decode-request-v1"
OBSERVATION_SCHEMA = "scorepeek-private-official-onnx-open-text-observations-v1"
DIAGNOSTIC_SCHEMA = "scorepeek-private-official-onnx-census-diagnostic-v1"
DIAGNOSTIC_STORE_MARKER = b'{"schema":"scorepeek-official-onnx-census-diagnostic-store-v1"}\n'
DIAGNOSTIC_MAX_BYTES = 4096
DECODER_TIMEOUT_SECONDS = 10 * 60
DECODE_BATCH_SIZE = 128
RENAME_NOREPLACE = 1
_FORWARDED_SIGNALS = (signal.SIGINT, signal.SIGTERM)


class OfficialCensusError(Exception):
    """The official ONNX song-identity census could not be completed."""

    def __init__(
        self,
        message: str,
        *,
        error_type: str = "census_error",
        status: str = "error",
    ) -> None:
        if status not in {"cancel", "error", "timeout"} or error_type not in {
            "cancel",
            "census_error",
            "decoder_diagnostics",
            "decoder_exit",
            "decoder_json",
            "decoder_output_bound",
            "decoder_process",
            "decoder_reap",
            "decoder_response",
            "decoder_timeout",
            "observation_publication",
        }:
            raise ValueError("official census error classification is invalid")
        super().__init__(message)
        self.error_type = error_type
        self.status = status


@dataclass(frozen=True)
class OfficialModelContract:
    model_id: str
    model_sha256: str
    dictionary_sha256: str
    preprocessor_id: str
    decode_schema: str


def _small_contract() -> OfficialModelContract:
    source = load_registered_onnx_source()
    return OfficialModelContract(
        source.model_id,
        source.sha256,
        source.paddle_inference_yml_sha256,
        PREPROCESSOR_ID,
        DECODE_SCHEMA,
    )


def _bundle_contract(model_id: str) -> OfficialModelContract:
    source = load_registered_onnx_bundle(model_id)
    files = {entry.filename: entry.sha256 for entry in source.files}
    return OfficialModelContract(
        source.model_id,
        files["inference.onnx"],
        files["inference.yml"],
        "paddleocr-3.7.0-bgr-dynamic-rec-resize-3x48x320-3200-v1",
        DYNAMIC_DECODE_SCHEMA,
    )


def _registered_contract(
    model_id: str,
    model_sha256: str,
    dictionary_sha256: str,
    preprocessor_id: str,
) -> OfficialModelContract:
    candidates = []
    small = _small_contract()
    if model_id == small.model_id:
        candidates.append(small)
    try:
        candidates.append(_bundle_contract(model_id))
    except ModelStoreError:
        pass
    matches = [
        contract
        for contract in candidates
        if contract.model_sha256 == model_sha256
        and contract.dictionary_sha256 == dictionary_sha256
        and contract.preprocessor_id == preprocessor_id
    ]
    if len(matches) != 1:
        raise OfficialCensusError("saved observation model binding is not registered")
    return matches[0]


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _validate_decoded_response(
    raw: Any,
    row_count: int,
    contract: OfficialModelContract | None = None,
    request_sha256: str | None = None,
) -> dict[str, Any]:
    contract = contract or _small_contract()
    required = {
        "schema",
        "model_id",
        "model_sha256",
        "dictionary_sha256",
        "preprocessor_id",
        "elapsed_ms",
        "decoded_text",
    }
    dynamic_required = required | {
        "request_sha256",
        "input_widths",
        "input_tensor_sha256s",
        "output_timesteps",
    }
    expected_fields = (
        dynamic_required
        if contract.decode_schema == DYNAMIC_DECODE_SCHEMA
        else required
    )
    if (
        not isinstance(raw, dict)
        or set(raw) != expected_fields
        or raw["schema"] != contract.decode_schema
        or raw["model_id"] != contract.model_id
        or raw["model_sha256"] != contract.model_sha256
        or raw["dictionary_sha256"] != contract.dictionary_sha256
        or raw["preprocessor_id"] != contract.preprocessor_id
        or type(raw["elapsed_ms"]) is not int
        or raw["elapsed_ms"] < 0
        or not isinstance(raw["decoded_text"], list)
        or len(raw["decoded_text"]) != row_count
        or any(not isinstance(value, str) for value in raw["decoded_text"])
    ):
        raise OfficialCensusError(
            "official ONNX decoder result is invalid",
            error_type="decoder_response",
        )
    if contract.decode_schema == DYNAMIC_DECODE_SCHEMA and (
        raw["request_sha256"] != request_sha256
        or any(
            not isinstance(raw[field], list) or len(raw[field]) != row_count
            for field in ("input_widths", "input_tensor_sha256s", "output_timesteps")
        )
        or any(
            type(value) is not int or not 320 <= value <= 3200
            for value in raw["input_widths"]
        )
        or any(
            type(value) is not int or value != width // 8
            for value, width in zip(
                raw["output_timesteps"], raw["input_widths"], strict=True
            )
        )
        or any(
            not isinstance(value, str)
            or len(value) != 64
            or any(character not in "0123456789abcdef" for character in value)
            for value in raw["input_tensor_sha256s"]
        )
    ):
        raise OfficialCensusError(
            "official ONNX dynamic decoder result is invalid",
            error_type="decoder_response",
        )
    return {
        key: raw[key]
        for key in (
            "schema",
            "model_id",
            "model_sha256",
            "dictionary_sha256",
            "preprocessor_id",
            "elapsed_ms",
            "decoded_text",
        )
    }


def _load_observations(
    path: Path,
    digest: str,
    training_input_sha256: str,
    catalog_candidates_sha256: str,
    labels: list[dict[str, Any]],
) -> dict[str, Any]:
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
    if not isinstance(raw, dict) or set(raw) != required:
        raise OfficialCensusError("saved observation bindings are invalid")
    contract = _registered_contract(
        raw["model_id"] if isinstance(raw["model_id"], str) else "",
        raw["model_sha256"] if isinstance(raw["model_sha256"], str) else "",
        (
            raw["dictionary_sha256"]
            if isinstance(raw["dictionary_sha256"], str)
            else ""
        ),
        raw["preprocessor_id"] if isinstance(raw["preprocessor_id"], str) else "",
    )
    if (
        raw["schema"] != OBSERVATION_SCHEMA
        or raw["training_input_sha256"] != training_input_sha256
        or raw["catalog_candidate_artifact_sha256"] != catalog_candidates_sha256
        or raw["model_id"] != contract.model_id
        or raw["model_sha256"] != contract.model_sha256
        or raw["dictionary_sha256"] != contract.dictionary_sha256
        or raw["preprocessor_id"] != contract.preprocessor_id
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
        "schema": contract.decode_schema,
        "model_id": contract.model_id,
        "model_sha256": contract.model_sha256,
        "dictionary_sha256": contract.dictionary_sha256,
        "preprocessor_id": contract.preprocessor_id,
        "elapsed_ms": None,
        "decoded_text": decoded_text,
    }


def _queries(text: str) -> tuple[str, ...]:
    return tuple(sorted({text, _exact_comparison_key(text), _comparison_key(text)}))


def _process_group_exists(process_group: int) -> bool:
    try:
        os.killpg(process_group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _terminate_process_group(process: subprocess.Popen[bytes], grace: float) -> bool:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + grace
    while _process_group_exists(process.pid) and time.monotonic() < deadline:
        process.poll()
        time.sleep(0.01)
    if _process_group_exists(process.pid):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=max(grace, 0.1))
    except subprocess.TimeoutExpired:
        return False
    deadline = time.monotonic() + grace
    while _process_group_exists(process.pid) and time.monotonic() < deadline:
        time.sleep(0.01)
    return not _process_group_exists(process.pid)


def _rename_noreplace(
    source_directory: int,
    source: str,
    destination_directory: int,
    destination: str,
) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    try:
        renameat2 = libc.renameat2
    except AttributeError as error:
        raise OSError("renameat2 is unavailable") from error
    renameat2.argtypes = (
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    )
    renameat2.restype = ctypes.c_int
    if (
        renameat2(
            source_directory,
            os.fsencode(source),
            destination_directory,
            os.fsencode(destination),
            RENAME_NOREPLACE,
        )
        != 0
    ):
        error_number = ctypes.get_errno()
        raise OSError(error_number, os.strerror(error_number), destination)


def _run_bounded(
    command: list[str],
    *,
    timeout: float = DECODER_TIMEOUT_SECONDS,
    stdout_limit: int = MAX_INPUT_BYTES,
    stderr_limit: int = MAX_MANIFEST_BYTES,
    termination_grace: float = 2.0,
) -> tuple[int, bytes, bytes]:
    previous_handlers: dict[signal.Signals, signal.Handlers] = {}
    interrupted_signal: int | None = None
    process: subprocess.Popen[bytes] | None = None
    streams: list[Any] = []
    selector: selectors.BaseSelector | None = None
    completed = False
    result: tuple[int, bytes, bytes] | None = None

    def interrupted(signum: int, _frame: object) -> None:
        nonlocal interrupted_signal
        interrupted_signal = signum

    def interruption_error() -> OfficialCensusError:
        return OfficialCensusError(
            f"official ONNX decoder interrupted by signal {interrupted_signal}",
            error_type="cancel",
            status="cancel",
        )

    buffers = [bytearray(), bytearray()]
    limits = [stdout_limit, stderr_limit]
    try:
        for selected in _FORWARDED_SIGNALS:
            previous_handlers[selected] = signal.signal(selected, interrupted)
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        streams = [process.stdout, process.stderr]
        if any(stream is None for stream in streams):
            raise OfficialCensusError(
                "official ONNX decoder pipes are unavailable",
                error_type="decoder_process",
            )
        if interrupted_signal is not None:
            raise interruption_error()
        selector = selectors.DefaultSelector()
        for index, stream in enumerate(streams):
            assert stream is not None
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, index)
        deadline = time.monotonic() + timeout
        failure: OfficialCensusError | None = None
        while process.poll() is None or selector.get_map():
            if interrupted_signal is not None:
                failure = interruption_error()
                break
            remaining_time = deadline - time.monotonic()
            if remaining_time <= 0:
                failure = OfficialCensusError(
                    "official ONNX decoder timed out",
                    error_type="decoder_timeout",
                    status="timeout",
                )
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
                    failure = OfficialCensusError(
                        "official ONNX decoder output exceeded its bound",
                        error_type="decoder_output_bound",
                    )
                    break
                buffers[index].extend(chunk)
            if failure is not None:
                break
        if failure is not None:
            raise failure
        if interrupted_signal is not None:
            raise interruption_error()
        if process.poll() is None:
            try:
                process.wait(timeout=max(deadline - time.monotonic(), 0.1))
            except subprocess.TimeoutExpired as error:
                raise OfficialCensusError(
                    "official ONNX decoder could not be reaped",
                    error_type="decoder_reap",
                ) from error
        if interrupted_signal is not None:
            raise interruption_error()
        if _process_group_exists(process.pid):
            try:
                group_removed = _terminate_process_group(process, termination_grace)
            except BaseException as error:
                raise OfficialCensusError(
                    "official ONNX decoder process group could not be removed",
                    error_type="decoder_reap",
                ) from error
            if not group_removed:
                raise OfficialCensusError(
                    "official ONNX decoder process group could not be removed",
                    error_type="decoder_reap",
                )
        completed = True
        result = process.returncode, bytes(buffers[0]), bytes(buffers[1])
    finally:
        cleanup_errors: list[BaseException] = []
        original_error = sys.exception()
        if process is not None and not completed:
            try:
                if not _terminate_process_group(
                    process, termination_grace
                ):
                    cleanup_errors.append(
                        OSError("decoder process group remains alive")
                    )
            except BaseException as error:
                cleanup_errors.append(error)
        if selector is not None:
            try:
                selector.close()
            except BaseException as error:
                cleanup_errors.append(error)
        for stream in streams:
            if stream is not None:
                try:
                    stream.close()
                except BaseException as error:
                    cleanup_errors.append(error)
        for selected, handler in previous_handlers.items():
            try:
                signal.signal(selected, handler)
            except BaseException as error:
                cleanup_errors.append(error)
        if cleanup_errors:
            raise OfficialCensusError(
                "official ONNX decoder resources could not be cleaned up",
                error_type="decoder_reap",
            ) from cleanup_errors[0]
        if original_error is None and interrupted_signal is not None:
            raise interruption_error()
    assert result is not None
    return result


class _DiagnosticRecorder:
    def __init__(self, path: Path | None, total_rows: int) -> None:
        self.path = path
        self.directory: int | None = None
        self.lock: int | None = None
        self.record: dict[str, Any] = {}
        self.available = False
        if (
            path is None
            or not path.is_absolute()
            or path.name in {"", ".", ".."}
            or not path.parent.is_dir()
        ):
            return
        parent: int | None = None
        initialization_lock: int | None = None
        staging_directory: int | None = None
        staging_name: str | None = None
        try:
            self.record = {
                "schema": DIAGNOSTIC_SCHEMA,
                "program": "scorepeek-ocr",
                "program_version": "0.0.0",
                "run_id": str(uuid.uuid4()),
                "status": "running",
                "completeness": "partial",
                "operation": "validate_inputs",
                "total_rows": total_rows,
                "completed_rows": 0,
                "batch_size": DECODE_BATCH_SIZE,
                "model_id": None,
                "error_type": None,
            }
            parent = os.open(
                path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
            )
            path_key = hashlib.sha256(os.fsencode(path.name)).hexdigest()[:16]
            initialization_lock = os.open(
                f".scorepeek-census-diagnostic-init-{path_key}.lock",
                os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,
                0o600,
                dir_fd=parent,
            )
            if not stat.S_ISREG(os.fstat(initialization_lock).st_mode):
                raise OSError("diagnostic initialization lock is not a regular file")
            fcntl.flock(
                initialization_lock,
                fcntl.LOCK_EX | fcntl.LOCK_NB,
            )
            os.fsync(parent)
            try:
                self.directory = os.open(
                    path.name,
                    os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                    dir_fd=parent,
                )
            except FileNotFoundError:
                staging_prefix = (
                    f".scorepeek-census-diagnostic-staging-{path_key}-"
                )
                if any(
                    entry.startswith(staging_prefix) for entry in os.listdir(parent)
                ):
                    raise OSError("diagnostic initialization is incomplete")
                staging_name = f"{staging_prefix}{uuid.uuid4()}"
                os.mkdir(staging_name, mode=0o700, dir_fd=parent)
                staging_directory = os.open(
                    staging_name,
                    os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                    dir_fd=parent,
                )
                marker = os.open(
                    ".scorepeek-owner.json",
                    os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                    0o600,
                    dir_fd=staging_directory,
                )
                try:
                    with os.fdopen(marker, "wb", closefd=False) as output:
                        output.write(DIAGNOSTIC_STORE_MARKER)
                        output.flush()
                        os.fsync(output.fileno())
                finally:
                    os.close(marker)
                os.fsync(staging_directory)
                _rename_noreplace(
                    parent,
                    staging_name,
                    parent,
                    path.name,
                )
                staging_name = None
                os.fsync(parent)
                self.directory = staging_directory
                staging_directory = None
            marker = os.open(
                ".scorepeek-owner.json",
                os.O_RDONLY | os.O_NOFOLLOW,
                dir_fd=self.directory,
            )
            try:
                marker_stat = os.fstat(marker)
                marker_bytes = os.read(marker, len(DIAGNOSTIC_STORE_MARKER) + 1)
            finally:
                os.close(marker)
            if (
                not stat.S_ISREG(marker_stat.st_mode)
                or marker_bytes != DIAGNOSTIC_STORE_MARKER
            ):
                raise OSError("diagnostic store ownership marker is invalid")
            self.lock = os.open(
                ".writer.lock",
                os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,
                0o600,
                dir_fd=self.directory,
            )
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            recovered = False
            for entry in os.listdir(self.directory):
                if entry.startswith(".snapshot-"):
                    os.unlink(entry, dir_fd=self.directory)
                    recovered = True
            if recovered:
                os.fsync(self.directory)
            self.available = True
        except OSError:
            self.close()
            return
        finally:
            if staging_directory is not None:
                try:
                    marker_stat = os.fstat(staging_directory)
                    if staging_name is not None:
                        current_stat = os.stat(
                            staging_name,
                            dir_fd=parent,
                            follow_symlinks=False,
                        )
                        if (
                            marker_stat.st_dev == current_stat.st_dev
                            and marker_stat.st_ino == current_stat.st_ino
                        ):
                            try:
                                os.unlink(
                                    ".scorepeek-owner.json",
                                    dir_fd=staging_directory,
                                )
                            except FileNotFoundError:
                                pass
                            os.rmdir(staging_name, dir_fd=parent)
                            os.fsync(parent)
                except OSError:
                    pass
                try:
                    os.close(staging_directory)
                except OSError:
                    pass
            if initialization_lock is not None:
                try:
                    fcntl.flock(initialization_lock, fcntl.LOCK_UN)
                except OSError:
                    pass
                try:
                    os.close(initialization_lock)
                except OSError:
                    pass
            if parent is not None:
                try:
                    os.close(parent)
                except OSError:
                    pass
        self._write()

    def update(self, **values: Any) -> None:
        self.record.update(values)
        self._write()

    def _write(self) -> None:
        if not self.available or self.directory is None:
            return
        temporary: str | None = None
        descriptor: int | None = None
        try:
            temporary = f".snapshot-{uuid.uuid4()}"
            descriptor = os.open(
                temporary,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                0o600,
                dir_fd=self.directory,
            )
            encoded = (
                json.dumps(
                    self.record,
                    separators=(",", ":"),
                    ensure_ascii=False,
                    allow_nan=False,
                )
                + "\n"
            ).encode()
            if len(encoded) > DIAGNOSTIC_MAX_BYTES:
                raise OSError("diagnostic snapshot exceeds its bound")
            with os.fdopen(descriptor, "wb") as output:
                descriptor = None
                output.write(encoded)
                output.flush()
                os.fsync(output.fileno())
            os.replace(
                temporary,
                "snapshot.json",
                src_dir_fd=self.directory,
                dst_dir_fd=self.directory,
            )
            os.fsync(self.directory)
        except OSError:
            self.available = False
            if temporary is not None:
                try:
                    os.unlink(temporary, dir_fd=self.directory)
                except OSError:
                    pass
        finally:
            if descriptor is not None:
                try:
                    os.close(descriptor)
                except OSError:
                    pass

    def close(self) -> None:
        if self.lock is not None:
            try:
                fcntl.flock(self.lock, fcntl.LOCK_UN)
            except OSError:
                pass
            try:
                os.close(self.lock)
            except OSError:
                pass
            self.lock = None
        if self.directory is not None:
            try:
                os.close(self.directory)
            except OSError:
                pass
            self.directory = None


def _record_failure(recorder: _DiagnosticRecorder, error: BaseException) -> None:
    if isinstance(error, KeyboardInterrupt):
        recorder.update(status="cancel", error_type="cancel")
    elif isinstance(error, OfficialCensusError):
        recorder.update(status=error.status, error_type=error.error_type)
    else:
        recorder.update(status="error", error_type="unexpected_error")


def _publish_observations(path: Path, data: bytes) -> None:
    descriptor: int | None = None
    staging: Path | None = None
    try:
        descriptor, raw_path = tempfile.mkstemp(
            prefix=f".{path.name}.staging-", dir=path.parent
        )
        staging = Path(raw_path)
        with os.fdopen(descriptor, "wb") as output:
            descriptor = None
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.link(staging, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        staging.unlink()
        staging = None
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except OSError as error:
        raise OfficialCensusError(
            "open-text observations could not be published",
            error_type="observation_publication",
        ) from error
    finally:
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass
        if staging is not None:
            try:
                staging.unlink(missing_ok=True)
            except OSError:
                pass


def _decode_rows(
    rows: list[tuple[str, str, str]],
    model: Path | None,
    dictionary: Path | None,
    bundle: Path | None,
    bundle_model_id: str | None,
    recorder: _DiagnosticRecorder,
) -> dict[str, Any]:
    if bundle is not None:
        if model is not None or dictionary is not None or bundle_model_id is None:
            raise OfficialCensusError("bundle cannot be combined with model arguments")
        contract = _bundle_contract(bundle_model_id)
    else:
        if model is None or dictionary is None or bundle_model_id is not None:
            raise OfficialCensusError("model and dictionary are required for ONNX inference")
        contract = _small_contract()
    recorder.update(operation="decode_batches", model_id=contract.model_id)
    decoded_text: list[str] = []
    elapsed_ms = 0
    with tempfile.TemporaryDirectory(
        prefix="scorepeek-official-onnx-census-"
    ) as temporary:
        for offset in range(0, len(rows), DECODE_BATCH_SIZE):
            batch = rows[offset : offset + DECODE_BATCH_SIZE]
            request = Path(temporary) / "request.json"
            request_bytes = (
                json.dumps(
                    {
                        "schema": REQUEST_SCHEMA,
                        "rows": [
                            {"path": path, "file_sha256": digest}
                            for path, _, digest in batch
                        ],
                    },
                    separators=(",", ":"),
                )
                + "\n"
            ).encode()
            request.write_bytes(request_bytes)
            command = [
                "cargo",
                "run",
                "--locked",
                "--quiet",
                "-p",
                "scorepeek",
                "--",
                "recognition",
            ]
            if bundle is not None:
                command.extend(
                    [
                        "title-official-dynamic-onnx-decode",
                        "--model-id",
                        contract.model_id,
                        "--bundle",
                        str(bundle),
                        "--request",
                        str(request),
                    ]
                )
            else:
                assert model is not None and dictionary is not None
                command.extend(
                    [
                        "title-official-onnx-decode",
                        "--model",
                        str(model),
                        "--dictionary",
                        str(dictionary),
                        "--request",
                        str(request),
                    ]
                )
            returncode, stdout, stderr = _run_bounded(command)
            if returncode != 0:
                raise OfficialCensusError(
                    f"official ONNX decoder failed with exit {returncode}: "
                    f"{stderr.decode(errors='replace').strip()[:8192]}",
                    error_type="decoder_exit",
                )
            if stderr:
                raise OfficialCensusError(
                    "official ONNX decoder emitted unexpected success diagnostics",
                    error_type="decoder_diagnostics",
                )
            try:
                response = _validate_decoded_response(
                    json.loads(stdout),
                    len(batch),
                    contract,
                    _sha256(request_bytes),
                )
            except json.JSONDecodeError as error:
                raise OfficialCensusError(
                    "official ONNX decoder returned invalid JSON",
                    error_type="decoder_json",
                ) from error
            decoded_text.extend(response["decoded_text"])
            elapsed_ms += response["elapsed_ms"]
            recorder.update(completed_rows=len(decoded_text))
    return {
        "schema": contract.decode_schema,
        "model_id": contract.model_id,
        "model_sha256": contract.model_sha256,
        "dictionary_sha256": contract.dictionary_sha256,
        "preprocessor_id": contract.preprocessor_id,
        "elapsed_ms": elapsed_ms,
        "decoded_text": decoded_text,
    }


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


def _run_census(
    preparation: Path,
    preparation_sha256: str,
    training_input: Path,
    training_input_sha256: str,
    catalog_candidates: Path,
    catalog_candidates_sha256: str,
    model: Path | None,
    dictionary: Path | None,
    bundle: Path | None,
    bundle_model_id: str | None,
    observations: Path | None,
    observations_sha256: str | None,
    output: Path,
    recorder: _DiagnosticRecorder,
    observation_output: Path | None = None,
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
    observation_output = observation_output or output.with_name(
        f"{output.name}.observations.json"
    )
    if (
        not observation_output.is_absolute()
        or observation_output.parent != output.parent
        or observation_output.exists()
        or observation_output == output
    ):
        raise OfficialCensusError(
            "observation output must be a new sibling of the census output"
        )
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
    recorder.update(total_rows=len(rows))

    reuse = observations is not None or observations_sha256 is not None
    try:
        if reuse:
            if (
                observations is None
                or observations_sha256 is None
                or model
                or dictionary
                or bundle
                or bundle_model_id
            ):
                raise OfficialCensusError(
                    "saved observations require their path and digest without model arguments"
                )
            recorder.update(operation="load_observations")
            decoded = _load_observations(
                observations,
                observations_sha256,
                training_input_sha256,
                catalog_candidates_sha256,
                labels,
            )
            recorder.update(
                completed_rows=len(rows), model_id=decoded["model_id"]
            )
        else:
            decoded = _decode_rows(
                rows, model, dictionary, bundle, bundle_model_id, recorder
            )
    except BaseException as error:
        _record_failure(recorder, error)
        raise

    observation_bytes = (
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
    recorder.update(operation="publish_observations")
    try:
        _publish_observations(observation_output, observation_bytes)
    except BaseException as error:
        _record_failure(recorder, error)
        raise

    recorder.update(operation="search_catalog")
    try:
        strategies = _model_record(
            decoded["decoded_text"],
            candidate_raw["candidates"],
            expected,
            labels,
            {split: len(rows_by_split[split]) for split in SPLITS},
        )
    except BaseException as error:
        _record_failure(recorder, error)
        raise
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
        "observation_sha256": _sha256(observation_bytes),
        "catalog_song_count": len(candidate_raw["candidates"]),
        "strategies": strategies,
    }
    recorder.update(operation="publish_artifact")
    staging = Path(
        tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent)
    )
    try:
        encoded = (
            json.dumps(record, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
            + "\n"
        ).encode()
        (staging / "observations.json").write_bytes(observation_bytes)
        (staging / "manifest.json").write_bytes(encoded)
        _publish(staging, output)
    except BaseException as error:
        if staging.exists():
            try:
                shutil.rmtree(staging)
            except OSError:
                pass
        _record_failure(recorder, error)
        raise
    recorder.update(
        operation="complete", status="success", completeness="complete"
    )
    return {
        **record,
        "artifact_sha256": _sha256(encoded),
        "diagnostic_recording": (
            "disabled"
            if recorder.path is None
            else "available"
            if recorder.available
            else "dropped"
        ),
    }


def run(
    preparation: Path,
    preparation_sha256: str,
    training_input: Path,
    training_input_sha256: str,
    catalog_candidates: Path,
    catalog_candidates_sha256: str,
    model: Path | None,
    dictionary: Path | None,
    bundle: Path | None,
    observations: Path | None,
    observations_sha256: str | None,
    output: Path,
    diagnostic_output: Path | None = None,
    observation_output: Path | None = None,
    bundle_model_id: str | None = None,
) -> dict[str, Any]:
    recorder = _DiagnosticRecorder(diagnostic_output, 0)
    try:
        return _run_census(
            preparation,
            preparation_sha256,
            training_input,
            training_input_sha256,
            catalog_candidates,
            catalog_candidates_sha256,
            model,
            dictionary,
            bundle,
            bundle_model_id,
            observations,
            observations_sha256,
            output,
            recorder,
            observation_output,
        )
    except BaseException as error:
        _record_failure(recorder, error)
        raise
    finally:
        recorder.close()


def main() -> None:
    parser = argparse.ArgumentParser()
    for name in ("preparation", "training-input", "catalog-candidates"):
        parser.add_argument(f"--{name}", type=Path, required=True)
        parser.add_argument(f"--{name}-sha256", required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--dictionary", type=Path)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--bundle-model-id")
    parser.add_argument("--observations", type=Path)
    parser.add_argument("--observations-sha256")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--observation-output", type=Path)
    recording = parser.add_mutually_exclusive_group()
    recording.add_argument("--diagnostic-output", type=Path)
    recording.add_argument("--no-recording", action="store_true")
    arguments = parser.parse_args()
    diagnostic_output = (
        None
        if arguments.no_recording
        else arguments.diagnostic_output
        or arguments.output.parent / ".scorepeek-official-census-diagnostic"
    )
    observation_output = arguments.observation_output or arguments.output.with_name(
        f"{arguments.output.name}.observations.json"
    )
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
            arguments.bundle,
            arguments.observations,
            arguments.observations_sha256,
            arguments.output,
            diagnostic_output,
            observation_output,
            arguments.bundle_model_id,
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
                "diagnostic_recording": result["diagnostic_recording"],
                "diagnostic_output": (
                    str(diagnostic_output) if diagnostic_output is not None else None
                ),
                "observation_output": str(observation_output),
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
