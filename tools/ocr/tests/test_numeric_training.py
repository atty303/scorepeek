from __future__ import annotations

import unittest

import numpy as np

from scorepeek_ocr.numeric_training import (
    DICTIONARY,
    FIELD_FAMILIES,
    NumericTrainingError,
    RUNTIME_FIELD_ORDER,
    _augment,
    _exact_ctc_candidates,
    _greedy_decode,
    _joint_score_decision,
    _matching_training_lineage,
    _register_crop_binding,
    _select_zero_error_calibration,
    _select_zero_error_temporal_calibration,
    _select_zero_error_temporal_joint_calibration,
    _select_zero_error_temporal_tuple_calibration,
    _valid_field_label,
)


class NumericTrainingTests(unittest.TestCase):
    @staticmethod
    def accepted_field(truth: str, candidates: list[tuple[str, float]]) -> dict:
        return {
            "truth": truth,
            "accepted": True,
            "calibration_accepted": True,
            "exact_ctc": {
                "candidates": [
                    {"text": text, "log_probability": probability}
                    for text, probability in candidates
                ],
                "all_blank_log_probability": -100.0,
            },
        }

    def test_every_runtime_numeric_field_has_one_calibration_family(self) -> None:
        self.assertEqual(set(RUNTIME_FIELD_ORDER), set(FIELD_FAMILIES))
        self.assertEqual(len(RUNTIME_FIELD_ORDER), 14)
        self.assertEqual(FIELD_FAMILIES["bad"], "judgment")
        self.assertEqual(FIELD_FAMILIES["combo_break"], "supplemental")

    def test_dataset_labels_match_runtime_field_grammar(self) -> None:
        self.assertTrue(_valid_field_label("notes", "007"))
        self.assertTrue(_valid_field_label("bad", "0"))
        self.assertTrue(_valid_field_label("miss_count", "--"))
        for field, label in (
            ("bad", "07"),
            ("level", "123"),
            ("combo_break", "1234"),
            ("miss_count", "-"),
            ("miss_count", "---"),
            ("current_score", "--"),
            ("unknown", "1"),
        ):
            self.assertFalse(_valid_field_label(field, label), (field, label))

    def test_crop_binding_rejects_label_and_split_reuse(self) -> None:
        digest = "0" * 64
        bindings: dict[str, tuple[str, str, str]] = {}
        sample = {
            "crop_sha256": digest,
            "field": "bad",
            "label": "0",
            "session_sha256": "1" * 64,
        }
        _register_crop_binding(bindings, sample)
        with self.assertRaisesRegex(NumericTrainingError, "evaluation splits"):
            _register_crop_binding(bindings, {**sample, "session_sha256": "2" * 64})
        with self.assertRaisesRegex(NumericTrainingError, "conflicting"):
            _register_crop_binding(bindings, {**sample, "label": "1"})

    def test_runtime_bundle_lineage_rejects_one_changed_input(self) -> None:
        lineage = {
            "initializer_manifest_sha256": "1" * 64,
            "initializer_checkpoint_sha256": "2" * 64,
            "training_source_commit": "3" * 40,
            "training_recipe": {"epochs": 18},
        }
        self.assertTrue(_matching_training_lineage(lineage, lineage))
        changed = {**lineage, "training_recipe": {"epochs": 17}}
        self.assertFalse(_matching_training_lineage(lineage, changed))

    def test_augmentation_is_deterministic_and_preserves_crop_shape(self) -> None:
        image = np.arange(6 * 11 * 3, dtype=np.uint8).reshape((6, 11, 3))
        first = _augment(image, 7)
        second = _augment(image, 7)
        self.assertEqual(len(first), 5)
        self.assertTrue(all(item.shape == image.shape for item in first))
        self.assertTrue(
            all(np.array_equal(left, right) for left, right in zip(first, second, strict=True))
        )

    def test_greedy_decode_uses_paddle_blank_first_token_order(self) -> None:
        probabilities = np.zeros((5, len(DICTIONARY) + 1), dtype=np.float32)
        probabilities[0, 1] = 1.0
        probabilities[1, 1] = 1.0
        probabilities[2, 0] = 1.0
        probabilities[3, 1] = 1.0
        probabilities[4, 2] = 1.0
        self.assertEqual(_greedy_decode(probabilities), "001")

    def test_greedy_decode_preserves_field_grammar_rejected_text_and_blank_tie(self) -> None:
        probabilities = np.zeros((7, len(DICTIONARY) + 1), dtype=np.float32)
        for row, token in enumerate((1, 0, 8, 8, 0, 11)):
            probabilities[row, token] = 1.0
        probabilities[6, 0] = 0.5
        probabilities[6, 1] = 0.5
        self.assertEqual(_greedy_decode(probabilities), "07-")

    def test_exact_ctc_preserves_repeat_separated_by_blank(self) -> None:
        probabilities = np.full((4, len(DICTIONARY) + 1), 1e-6, dtype=np.float32)
        probabilities[0, 2] = 1.0
        probabilities[1, 0] = 1.0
        probabilities[2, 2] = 1.0
        probabilities[3, 0] = 1.0
        probabilities /= probabilities.sum(axis=1, keepdims=True)
        ranked = _exact_ctc_candidates(probabilities, "level", 1.0)
        self.assertEqual(ranked["candidates"][0]["text"], "11")
        self.assertGreater(
            ranked["candidates"][0]["log_probability"],
            ranked["all_blank_log_probability"],
        )

    def test_exact_ctc_does_not_force_blank_crop_to_zero(self) -> None:
        probabilities = np.full((3, len(DICTIONARY) + 1), 1e-6, dtype=np.float32)
        probabilities[:, 0] = 1.0
        probabilities /= probabilities.sum(axis=1, keepdims=True)
        ranked = _exact_ctc_candidates(probabilities, "bad", 1.0)
        self.assertGreater(
            ranked["all_blank_log_probability"],
            ranked["candidates"][0]["log_probability"],
        )

    def test_dash_is_only_in_display_field_grammar(self) -> None:
        probabilities = np.full((3, len(DICTIONARY) + 1), 1e-6, dtype=np.float32)
        probabilities[0, 11] = 1.0
        probabilities[1, 0] = 1.0
        probabilities[2, 11] = 1.0
        probabilities /= probabilities.sum(axis=1, keepdims=True)
        display = _exact_ctc_candidates(probabilities, "miss_count", 1.0)
        numeric = _exact_ctc_candidates(probabilities, "bad", 1.0)
        self.assertEqual(display["candidates"][0]["text"], "--")
        self.assertNotEqual(numeric["candidates"][0]["text"], "--")

    def test_leading_zero_is_only_admitted_for_notes(self) -> None:
        probabilities = np.full((3, len(DICTIONARY) + 1), 1e-6, dtype=np.float32)
        probabilities[0, 1] = 1.0
        probabilities[1, 0] = 1.0
        probabilities[2, 8] = 1.0
        probabilities /= probabilities.sum(axis=1, keepdims=True)
        notes = _exact_ctc_candidates(probabilities, "notes", 1.0)
        bad = _exact_ctc_candidates(probabilities, "bad", 1.0)
        self.assertIn("07", {candidate["text"] for candidate in notes["candidates"]})
        self.assertNotIn("07", {candidate["text"] for candidate in bad["candidates"]})

    def test_calibration_prefers_coverage_but_never_accepts_wrong_top_one(self) -> None:
        def observation(probability: float, margin: float, correct: bool) -> dict:
            return {
                "candidates": [{"calibrated_probability": probability, "log_probability": -1.0}],
                "all_blank_log_probability": -2.0,
                "runner_up_margin": margin,
                "correct": correct,
            }

        rows = []
        for probability, margin, correct in [
            (0.9, 2.0, True),
            (0.8, 1.5, True),
            (0.7, 0.5, False),
        ]:
            rows.append(
                {
                    "field": "bad",
                    "temperatures": {
                        str(temperature): observation(probability, margin, correct)
                        for temperature in (0.75, 1.0, 1.25, 1.5, 2.0)
                    },
                }
            )
        selected = _select_zero_error_calibration(rows, "judgment")
        self.assertEqual(selected["accepted_correct"], 2)
        self.assertEqual(selected["accepted_incorrect"], 0)

    def test_joint_score_can_precede_catalog_notes(self) -> None:
        decision = _joint_score_decision(
            {
                "current_score": self.accepted_field("1383", [("1303", -0.1), ("1383", -0.2)]),
                "pgreat": self.accepted_field("630", [("630", -0.1)]),
                "great": self.accepted_field("123", [("123", -0.1)]),
            }
        )
        self.assertIsNotNone(decision)
        assert decision is not None
        self.assertTrue(decision["correct"])
        self.assertEqual(decision["candidates"][0]["current_score"], 1383)

    def test_joint_score_requires_pgreat_and_great_judgment_calibration(self) -> None:
        pgreat = self.accepted_field("630", [("630", -0.1)])
        pgreat["calibration_accepted"] = False
        decision = _joint_score_decision(
            {
                "current_score": self.accepted_field("1383", [("1383", -0.1)]),
                "pgreat": pgreat,
                "great": self.accepted_field("123", [("123", -0.1)]),
            }
        )
        self.assertIsNone(decision)

    def test_temporal_calibration_excludes_repeated_wrong_values(self) -> None:
        predictions = []
        for episode, text, truth, probability in [
            ("correct", "7", "7", 0.20),
            ("wrong", "1", "7", 0.05),
        ]:
            for sequence in (1, 2):
                predictions.append(
                    {
                        "session_sha256": "0" * 64,
                        "episode_id": episode,
                        "sequence": sequence,
                        "field": "good",
                        "truth": truth,
                        "correct": text == truth,
                        "exact_ctc": {
                            "candidates": [
                                {"text": text, "log_probability": -1.0, "calibrated_probability": probability},
                                {"text": "9", "log_probability": -2.0, "calibrated_probability": 0.01},
                            ],
                            "all_blank_log_probability": -3.0,
                            "runner_up_margin": 1.0,
                        },
                    }
                )
        selected = _select_zero_error_temporal_calibration(predictions, "judgment", 2.0)
        self.assertIsNotNone(selected)
        assert selected is not None
        self.assertEqual(selected["accepted_correct"], 1)
        self.assertEqual(selected["accepted_incorrect"], 0)
        self.assertGreater(selected["minimum_probability"], 0.05)

    def test_temporal_joint_calibration_excludes_repeated_wrong_tuple(self) -> None:
        def decision(score: int, correct: bool, margin: float) -> dict[str, object]:
            return {
                "candidates": [
                    {"current_score": score, "pgreat": score // 2, "great": 0}
                ],
                "runner_up_margin": margin,
                "correct": correct,
            }

        rows = [
            (("session", "correct", 1), decision(100, True, 0.4)),
            (("session", "correct", 2), decision(100, True, 0.4)),
            (("session", "wrong", 1), decision(102, False, 0.1)),
            (("session", "wrong", 2), decision(102, False, 0.1)),
        ]
        selected = _select_zero_error_temporal_joint_calibration(rows)
        self.assertIsNotNone(selected)
        assert selected is not None
        self.assertEqual(selected["accepted_correct"], 1)
        self.assertEqual(selected["accepted_incorrect"], 0)

    def test_temporal_judgment_tuple_calibration_is_zero_error(self) -> None:
        fields = {"pgreat", "great", "good", "bad", "poor"}
        predictions = []
        for episode, correct, margin in (("correct", True, 1.5), ("wrong", False, 0.2)):
            for sequence in (1, 2):
                for index, field in enumerate(sorted(fields)):
                    text = str(index + (0 if correct else 5))
                    predictions.append(
                        {
                            "session_sha256": "0" * 64,
                            "episode_id": episode,
                            "sequence": sequence,
                            "field": field,
                            "correct": correct,
                            "exact_ctc": {
                                "candidates": [
                                    {
                                        "text": text,
                                        "log_probability": -1.0,
                                        "calibrated_probability": 0.8,
                                    }
                                ],
                                "all_blank_log_probability": -3.0,
                                "runner_up_margin": margin,
                            },
                        }
                    )
        selected = _select_zero_error_temporal_tuple_calibration(
            predictions, fields, 1.0
        )
        self.assertIsNotNone(selected)
        assert selected is not None
        self.assertEqual(selected["accepted_correct"], 1)
        self.assertEqual(selected["accepted_incorrect"], 0)
        self.assertGreater(selected["minimum_runner_up_margin"], 0.2)


if __name__ == "__main__":
    unittest.main()
