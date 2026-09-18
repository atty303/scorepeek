"""Versioned presentation transforms for offline title-model experiments."""

from __future__ import annotations

import cv2
import numpy as np

IDENTITY_TRANSFORM_ID = "scorepeek-title-rgb-identity-v1"
CHANNEL_MAX_TRANSFORM_ID = "scorepeek-title-channel-max-rgb-v1"
TRANSFORM_IDS = (IDENTITY_TRANSFORM_ID, CHANNEL_MAX_TRANSFORM_ID)


class TitlePresentationError(Exception):
    """A title crop could not be transformed under the selected contract."""


def apply_transform(image: np.ndarray, transform_id: str) -> np.ndarray:
    """Apply one registered transform to a decoded RGB/BGR title crop."""
    if image.ndim != 3 or image.shape[2] != 3 or image.dtype != np.uint8:
        raise TitlePresentationError("title presentation input must be uint8 RGB or BGR")
    if transform_id == IDENTITY_TRANSFORM_ID:
        return image
    if transform_id != CHANNEL_MAX_TRANSFORM_ID:
        raise TitlePresentationError("title presentation transform is not registered")
    maximum = image.max(axis=2, keepdims=True)
    return np.repeat(maximum, 3, axis=2)


def transform_crop_bytes(data: bytes, transform_id: str) -> bytes:
    """Decode an image, apply a registered transform, and encode a P6 crop."""
    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None:
        raise TitlePresentationError("title presentation crop could not be decoded")
    encoded, output = cv2.imencode(".ppm", apply_transform(image, transform_id))
    if not encoded:
        raise TitlePresentationError("title presentation crop could not be encoded")
    return output.tobytes()
