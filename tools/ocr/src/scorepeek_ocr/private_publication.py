"""Serialize create-only private artifact directory publication."""

from __future__ import annotations

import fcntl
import os
import stat
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

LOCK_NAME = ".scorepeek-private-output.lock"


class PrivatePublicationError(Exception):
    """A private artifact directory could not be published create-only."""


@contextmanager
def publication_lock(parent: Path) -> Iterator[None]:
    flags = os.O_CREAT | os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW
    descriptor = os.open(parent / LOCK_NAME, flags, 0o600)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise PrivatePublicationError("private publication lock is not a regular file")
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def destination_exists(path: Path) -> bool:
    return os.path.lexists(path)
