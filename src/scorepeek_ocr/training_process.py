"""Run bounded offline training subprocesses with explicit process ownership."""

from __future__ import annotations

import os
import signal
import subprocess
import time
from collections.abc import Mapping, Sequence
from pathlib import Path

TERMINATION_GRACE_SECONDS = 5
POLL_INTERVAL_SECONDS = 0.01
_FORWARDED_SIGNALS = (signal.SIGINT, signal.SIGTERM)


class TrainingProcessError(Exception):
    """A bounded offline training subprocess did not complete successfully."""


def _group_exists(process_group: int) -> bool:
    try:
        os.killpg(process_group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _wait_for_group_exit(
    process: subprocess.Popen[bytes], process_group: int, timeout: float
) -> bool:
    deadline = time.monotonic() + timeout
    while True:
        process.poll()
        if not _group_exists(process_group):
            return True
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        time.sleep(min(POLL_INTERVAL_SECONDS, remaining))


def _stop_group(process: subprocess.Popen[bytes], process_group: int) -> None:
    if _group_exists(process_group):
        try:
            os.killpg(process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass
    if not _wait_for_group_exit(process, process_group, TERMINATION_GRACE_SECONDS):
        try:
            os.killpg(process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass
        if not _wait_for_group_exit(
            process, process_group, TERMINATION_GRACE_SECONDS
        ):
            raise TrainingProcessError("subprocess group could not be terminated")
    if process.poll() is None:
        try:
            process.wait(timeout=TERMINATION_GRACE_SECONDS)
        except subprocess.TimeoutExpired as error:
            raise TrainingProcessError("subprocess leader could not be reaped") from error


def run_checked(
    arguments: Sequence[str],
    *,
    cwd: Path | None = None,
    environment: Mapping[str, str] | None = None,
    timeout_seconds: int,
) -> None:
    if timeout_seconds <= 0:
        raise TrainingProcessError("subprocess timeout must be positive")

    previous_handlers: dict[signal.Signals, signal.Handlers] = {}
    process: subprocess.Popen[bytes] | None = None
    interrupted_signal: int | None = None

    def interrupted(signum: int, _frame: object) -> None:
        nonlocal interrupted_signal
        interrupted_signal = signum

    try:
        for selected in _FORWARDED_SIGNALS:
            previous_handlers[selected] = signal.signal(selected, interrupted)
        process = subprocess.Popen(
            list(arguments),
            cwd=cwd,
            env=None if environment is None else dict(environment),
            start_new_session=True,
        )

        failure: BaseException | None = None
        return_code: int | None = None
        deadline = time.monotonic() + timeout_seconds
        while return_code is None and failure is None:
            if interrupted_signal is not None:
                failure = TrainingProcessError(
                    f"subprocess interrupted by signal {interrupted_signal}"
                )
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                failure = TrainingProcessError("subprocess timed out")
                break
            try:
                return_code = process.wait(
                    timeout=min(POLL_INTERVAL_SECONDS, remaining)
                )
            except subprocess.TimeoutExpired:
                pass
            except BaseException as error:
                failure = error

        if failure is None and return_code != 0:
            failure = TrainingProcessError(
                f"subprocess exited with status {return_code}"
            )
        try:
            _stop_group(process, process.pid)
        except BaseException as cleanup_error:
            if failure is None:
                raise
            raise TrainingProcessError(
                "subprocess failed and its process group could not be terminated"
            ) from cleanup_error
        process = None
        if failure is not None:
            raise failure
    finally:
        cleanup_error: BaseException | None = None
        if process is not None:
            try:
                _stop_group(process, process.pid)
            except BaseException as error:
                cleanup_error = error
        for selected, handler in previous_handlers.items():
            signal.signal(selected, handler)
        if cleanup_error is not None:
            raise TrainingProcessError(
                "subprocess process group could not be terminated"
            ) from cleanup_error
