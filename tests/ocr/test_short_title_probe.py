from __future__ import annotations

import math
import shutil
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import cv2
import numpy as np

from scorepeek_ocr.training_artifacts import _read as _read_artifact
from scorepeek_ocr.short_title_probe import (
    HORIZONTAL_VIEW,
    ORIGINAL_VIEW,
    TIGHT_VIEW,
    Catalog,
    Label,
    _catalog_ranking,
    _decode_image,
    _encode_catalog,
    _foreground,
    _load_catalog,
    _sha256,
    _snapshot_verified_file,
    _single_token_scores,
    _view,
)


class ShortTitleProbeTests(unittest.TestCase):
    @staticmethod
    def _catalog(path: Path, title: str) -> None:
        connection = sqlite3.connect(path)
        try:
            connection.executescript(
                "CREATE TABLE songs (song_id TEXT PRIMARY KEY);"
                "CREATE TABLE title_variants ("
                "song_id TEXT, source_id TEXT, evidence_digest TEXT, "
                "variant_kind TEXT, value TEXT);"
            )
            connection.execute("INSERT INTO songs VALUES ('song-1')")
            connection.execute(
                "INSERT INTO title_variants VALUES "
                "('song-1','source','digest','in_game_display',?)",
                (title,),
            )
            connection.commit()
        finally:
            connection.close()

    def test_registered_views_apply_the_observed_fixed_geometry(self) -> None:
        image = np.zeros((45, 475, 3), dtype=np.uint8)
        image[5:9, 10:13] = 255
        box = _foreground(image)
        self.assertEqual(box, (10, 5, 3, 4))
        self.assertEqual(_view(image, box, ORIGINAL_VIEW).shape, (45, 475, 3))
        self.assertEqual(_view(image, box, TIGHT_VIEW).shape, (6, 25, 3))
        self.assertEqual(_view(image, box, HORIZONTAL_VIEW).shape, (45, 11, 3))

    def test_single_token_dynamic_program_matches_ctc_probability(self) -> None:
        probabilities = np.asarray(
            [[0.6, 0.3, 0.1], [0.5, 0.4, 0.1]], dtype=np.float32
        )
        scores = _single_token_scores(probabilities)
        from scorepeek_ocr.parity import ctc_log_probability

        self.assertAlmostEqual(
            float(scores[0]),
            math.exp(ctc_log_probability(probabilities, [1])),
            places=7,
        )
        self.assertAlmostEqual(
            float(scores[1]),
            math.exp(ctc_log_probability(probabilities, [2])),
            places=7,
        )

    def test_catalog_query_uses_the_verified_snapshot_after_path_replacement(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / "catalog.sqlite3"
            replacement = root / "replacement.sqlite3"
            self._catalog(original, "OLD")
            self._catalog(replacement, "NEW")
            digest = _sha256(original.read_bytes())

            def snapshot_then_replace(
                path: Path, expected: str, maximum: int, snapshot: Path
            ) -> int:
                size = _snapshot_verified_file(path, expected, maximum, snapshot)
                shutil.copyfile(replacement, original)
                return size

            with mock.patch(
                "scorepeek_ocr.short_title_probe._snapshot_verified_file",
                side_effect=snapshot_then_replace,
            ):
                catalog = _load_catalog(original, digest)
            self.assertIn("OLD", catalog.title_songs)
            self.assertNotIn("NEW", catalog.title_songs)

    def test_crop_decode_uses_the_same_verified_bytes(self) -> None:
        image = np.zeros((4, 5, 3), dtype=np.uint8)
        image[1:3, 2:4] = 255
        encoded, ppm = cv2.imencode(".ppm", image)
        self.assertTrue(encoded)
        data = ppm.tobytes()
        pixels = data.split(b"\n", 3)[3]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "crop.ppm"
            path.write_bytes(data)
            label = Label(
                group_id="group-1",
                song_id="song-1",
                title="X",
                crop_file_sha256=_sha256(data),
                crop_pixel_sha256=_sha256(pixels),
                path=path,
            )
            def read_then_replace(*args, **kwargs):
                verified = _read_artifact(*args, **kwargs)
                path.write_bytes(b"changed")
                return verified

            with mock.patch(
                "scorepeek_ocr.short_title_probe._read_artifact",
                side_effect=read_then_replace,
            ):
                decoded = _decode_image(label)
            self.assertEqual(decoded.shape, image.shape)

    def test_catalog_audit_and_alias_ranking_keep_all_scoreable_songs_competing(
        self,
    ) -> None:
        shime = "00000000-0000-0000-0000-000000000001"
        letter_x = "00000000-0000-0000-0000-000000000002"
        unsupported = "00000000-0000-0000-0000-000000000003"
        catalog = Catalog(
            variants_by_song={
                shime: ("〆",),
                letter_x: ("X",),
                unsupported: ("Ω",),
            },
            title_songs={
                "〆": frozenset({shime}),
                "X": frozenset({letter_x}),
                "Ω": frozenset({unsupported}),
            },
        )
        indexes = {"〆": 1, "X": 2, "x": 3}
        encoded, unsupported_variants, unencodable_songs = _encode_catalog(
            catalog, indexes
        )
        self.assertEqual(unsupported_variants, 1)
        self.assertEqual(unencodable_songs, 1)

        probabilities = np.asarray(
            [[0.2, 0.1, 0.25, 0.45], [0.8, 0.05, 0.05, 0.1]], dtype=np.float32
        )
        result = _catalog_ranking(
            probabilities,
            encoded,
            shime,
            catalog,
            shime,
            [indexes["x"]],
        )
        self.assertEqual(result["without_alias"]["scoreable_song_count"], 2)
        self.assertEqual(result["without_alias"]["unscoreable_song_count"], 1)
        self.assertEqual(result["with_alias"]["scoreable_song_count"], 2)
        self.assertEqual(result["with_alias"]["truth_rank"], 1)


if __name__ == "__main__":
    unittest.main()
