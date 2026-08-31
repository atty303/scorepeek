from __future__ import annotations

import hashlib
import unittest

import cv2
import numpy as np

from scorepeek_ocr.fixed_slot_training import (
    CLASSES,
    FEATURE_DIMENSIONS,
    feature,
    hard_mask,
    has_not_displayed_marker,
    soft_mask,
)


class FixedSlotTrainingTests(unittest.TestCase):
    def test_fixed_slot_feature_contract_is_finite_and_normalized(self) -> None:
        image = np.zeros((32, 24, 3), dtype=np.uint8)
        image[5:27, 8:16] = (220, 220, 220)
        value = feature(image, "pgreat")
        self.assertEqual(CLASSES, "_0123456789")
        self.assertEqual(value.shape, (FEATURE_DIMENSIONS,))
        self.assertTrue(np.isfinite(value).all())
        self.assertAlmostEqual(float(np.linalg.norm(value)), 1.0, places=6)

    def test_synthetic_mask_resize_and_feature_match_rust_reference(self) -> None:
        image = np.zeros((22, 27, 3), dtype=np.uint8)
        for y in range(22):
            for x in range(27):
                image[y, x] = 230 if (x // 3 + y // 4) % 2 == 0 else 20
        hard = hard_mask(image, "pgreat")
        soft = soft_mask(image, "pgreat")
        self.assertEqual(
            hashlib.sha256(hard.tobytes()).hexdigest(),
            "04e2a96647d744fc1b3992c879cfe536f424dd3191dd7600dd0cf1d50d63bac1",
        )
        self.assertEqual(
            hashlib.sha256(soft.tobytes()).hexdigest(),
            "d711599e6e4da839b7bf49b3de9c22d7e090f11d32c5b9b6a9e24edde8fb02bf",
        )
        self.assertEqual(
            hashlib.sha256(
                cv2.resize(hard, (24, 32), interpolation=cv2.INTER_LINEAR).tobytes()
            ).hexdigest(),
            "5218a105aabdd46c57388448979d1b81f8b4b5d678c295a3c28203fbcfa9b651",
        )
        self.assertEqual(
            hashlib.sha256(
                cv2.resize(soft, (24, 32), interpolation=cv2.INTER_LINEAR).tobytes()
            ).hexdigest(),
            "59c5e8a9bac8a8703c2660e1a80e857d7cd009df59d4b1c9a456ca64da1abe52",
        )
        np.testing.assert_allclose(
            feature(image, "pgreat")[:5],
            [0.1558995843, 0.0148657886, 0.0112390490, 0.0243966635, 0.0758169368],
            rtol=0.0,
            atol=1e-7,
        )

    def test_field_family_masks_keep_expected_colors_separate(self) -> None:
        image = np.zeros((6, 10, 3), dtype=np.uint8)
        image[2:4, 1:3] = (200, 180, 120)  # current-score cyan in BGR
        image[2:4, 4:6] = (50, 100, 230)  # level red in BGR
        image[2:4, 7:9] = (210, 210, 210)  # neutral judgment white

        self.assertEqual(hard_mask(image, "current_score")[3, 2], 255)
        self.assertEqual(hard_mask(image, "level")[3, 5], 255)
        self.assertEqual(hard_mask(image, "pgreat")[3, 8], 255)
        self.assertGreater(soft_mask(image, "current_score")[2, 1], 0)
        self.assertGreater(soft_mask(image, "level")[2, 4], 0)
        self.assertGreater(soft_mask(image, "pgreat")[2, 7], 0)

    def test_not_displayed_marker_requires_fixed_horizontal_geometry(self) -> None:
        marker = np.zeros((55, 150, 3), dtype=np.uint8)
        marker[32:34, 20:94] = 220
        detected, metrics = has_not_displayed_marker(marker)
        self.assertTrue(detected)
        self.assertEqual(
            metrics,
            {"maximum_row": 74, "long_rows": 2, "occupied_columns": 74},
        )

        marker[33, 94:120] = 220
        self.assertFalse(has_not_displayed_marker(marker)[0])


if __name__ == "__main__":
    unittest.main()
