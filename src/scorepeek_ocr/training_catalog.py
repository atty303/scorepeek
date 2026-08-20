"""Score private training crops against every catalog title through a CTC trie."""

from __future__ import annotations

import math
import time
from dataclasses import dataclass
from typing import Any

import numpy as np
import paddle

from scorepeek_ocr.training_initializer import _decode, _preprocess
from scorepeek_ocr.title_presentation import IDENTITY_TRANSFORM_ID


class TrainingCatalogError(Exception):
    """Catalog-constrained training evaluation could not be completed."""


@dataclass(frozen=True)
class CatalogDecisions:
    correct: tuple[bool, ...]
    margins: tuple[float, ...]
    expected_song_ids: tuple[str, ...]


class CatalogTrie:
    """A vectorized equivalent of the runtime's exact catalog-title CTC trie."""

    def __init__(
        self,
        candidates: list[dict[str, Any]],
        tokens: list[str],
        maximum_timesteps: int,
    ) -> None:
        indexes: dict[str, int] = {}
        duplicates = set()
        for index, token in enumerate(tokens[1:], 1):
            if len(token) != 1:
                continue
            if token in indexes:
                duplicates.add(token)
            else:
                indexes[token] = index
        for duplicate in duplicates:
            indexes.pop(duplicate)

        parents = [0]
        node_tokens = [0]
        children: list[dict[int, int]] = [{}]
        terminal_nodes = []
        terminal_songs = []
        song_ids = []
        for song_index, candidate in enumerate(candidates):
            song_ids.append(candidate["song_id"])
            for variant in candidate["variants"]:
                try:
                    sequence = [indexes[character] for character in variant["value"]]
                except KeyError as error:
                    raise TrainingCatalogError(
                        "catalog title is outside the prepared dictionary"
                    ) from error
                required = len(sequence) + sum(
                    left == right
                    for left, right in zip(sequence, sequence[1:], strict=False)
                )
                if not sequence or required > maximum_timesteps:
                    raise TrainingCatalogError(
                        "catalog title is outside the prepared timestep contract"
                    )
                node = 0
                for token in sequence:
                    child = children[node].get(token)
                    if child is None:
                        child = len(parents)
                        children[node][token] = child
                        parents.append(node)
                        node_tokens.append(token)
                        children.append({})
                    node = child
                terminal_nodes.append(node)
                terminal_songs.append(song_index)
        if not song_ids or len(set(song_ids)) != len(song_ids):
            raise TrainingCatalogError("catalog song IDs are empty or duplicated")
        self.song_ids = tuple(song_ids)
        self._song_indexes = {song_id: index for index, song_id in enumerate(song_ids)}
        self._parents = np.asarray(parents, dtype=np.int64)
        self._tokens = np.asarray(node_tokens, dtype=np.int64)
        self._terminal_nodes = np.asarray(terminal_nodes, dtype=np.int64)
        self._terminal_songs = np.asarray(terminal_songs, dtype=np.int64)

    def score(self, probabilities: np.ndarray) -> np.ndarray:
        if (
            probabilities.ndim != 2
            or probabilities.shape[1] <= int(self._tokens.max())
            or not np.isfinite(probabilities).all()
            or np.any(probabilities <= 0)
        ):
            raise TrainingCatalogError("model output is not a positive finite CTC tensor")
        sums = probabilities.sum(axis=1)
        if np.max(np.abs(sums - 1.0)) > 1e-3:
            raise TrainingCatalogError("model output is not a normalized CTC tensor")
        probabilities = probabilities / sums[:, None]

        blank = np.full(len(self._parents), -np.inf)
        nonblank = np.full(len(self._parents), -np.inf)
        blank[0] = 0.0
        parent_tokens = self._tokens[self._parents]
        may_skip_parent_nonblank = (self._tokens != parent_tokens) | (self._parents == 0)
        for row in probabilities:
            blank_log_probability = math.log(float(row[0]))
            next_blank = np.logaddexp(blank, nonblank) + blank_log_probability
            next_blank[0] = blank[0] + blank_log_probability
            from_parent = np.where(
                may_skip_parent_nonblank,
                np.logaddexp(blank[self._parents], nonblank[self._parents]),
                blank[self._parents],
            )
            next_nonblank = (
                np.logaddexp(nonblank, from_parent) + np.log(row[self._tokens])
            )
            next_nonblank[0] = -np.inf
            blank, nonblank = next_blank, next_nonblank
        node_scores = np.logaddexp(blank, nonblank)
        song_scores = np.full(len(self.song_ids), -np.inf)
        np.maximum.at(
            song_scores,
            self._terminal_songs,
            node_scores[self._terminal_nodes],
        )
        return song_scores

    def expected_indexes(self, song_ids: list[str]) -> list[int]:
        try:
            return [self._song_indexes[song_id] for song_id in song_ids]
        except KeyError as error:
            raise TrainingCatalogError("training truth is absent from the candidate catalog") from error


def training_truth(
    rows: list[tuple[str, str, str]], labels: list[dict[str, Any]]
) -> list[str]:
    by_digest = {label["crop_file_sha256"]: label["song_id"] for label in labels}
    if len(by_digest) != len(labels):
        raise TrainingCatalogError("training truth repeats a crop file digest")
    try:
        truth = [by_digest[digest] for _, _, digest in rows]
    except KeyError as error:
        raise TrainingCatalogError("prepared rows and training truth differ") from error
    if len(rows) != len(labels):
        raise TrainingCatalogError("prepared rows and training truth counts differ")
    return truth


def evaluate_catalog(
    model,
    rows: list[tuple[str, str, str]],
    expected_song_ids: list[str],
    tokens: list[str],
    width: int,
    trie: CatalogTrie,
    presentation_transform_id: str = IDENTITY_TRANSFORM_ID,
) -> tuple[dict[str, Any], CatalogDecisions]:
    if len(rows) != len(expected_song_ids):
        raise TrainingCatalogError("catalog evaluation truth count differs from rows")
    expected_indexes = trie.expected_indexes(expected_song_ids)
    started = time.perf_counter()
    correct = []
    margins = []
    strict = 0
    model.eval()
    with paddle.no_grad():
        for offset in range(0, len(rows), 8):
            batch = rows[offset : offset + 8]
            images = np.stack(
                [
                    _preprocess(path, width, digest, presentation_transform_id)
                    for path, _, digest in batch
                ]
            )
            outputs = model(paddle.to_tensor(images)).numpy()
            for local, (probabilities, (_, title, _)) in enumerate(
                zip(outputs, batch, strict=True)
            ):
                scores = trie.score(probabilities)
                top_two = np.argpartition(scores, -2)[-2:]
                ranked = top_two[np.argsort(scores[top_two])[::-1]]
                top = int(ranked[0])
                margin = float(scores[ranked[0]] - scores[ranked[1]])
                correct.append(
                    margin > 0 and top == expected_indexes[offset + local]
                )
                margins.append(margin)
                strict += _decode(probabilities, tokens) == title
    correct_margins = [margin for match, margin in zip(correct, margins, strict=True) if match]
    incorrect_margins = [margin for match, margin in zip(correct, margins, strict=True) if not match]
    songs = set(expected_song_ids)
    fully_correct_songs = {
        song_id
        for song_id in songs
        if all(
            match
            for match, expected_song_id in zip(
                correct, expected_song_ids, strict=True
            )
            if expected_song_id == song_id
        )
    }
    probe = {
        "sample_count": len(rows),
        "song_count": len(songs),
        "fully_correct_song_count": len(fully_correct_songs),
        "correct_unique_song_id_decision_count": sum(correct),
        "incorrect_or_tied_song_id_decision_count": len(rows) - sum(correct),
        "strict_open_text_count": strict,
        "minimum_correct_runner_up_margin": min(correct_margins, default=None),
        "maximum_incorrect_runner_up_margin": max(incorrect_margins, default=None),
        "elapsed_ms": round((time.perf_counter() - started) * 1000),
    }
    return probe, CatalogDecisions(
        tuple(correct), tuple(margins), tuple(expected_song_ids)
    )


def improves_catalog_identity(
    baseline: CatalogDecisions, candidate: CatalogDecisions
) -> bool:
    if len(baseline.correct) != len(candidate.correct):
        raise TrainingCatalogError("catalog decision counts differ")
    if baseline.expected_song_ids != candidate.expected_song_ids:
        raise TrainingCatalogError("catalog decision truth differs")
    if not all(
        not before or after
        for before, after in zip(baseline.correct, candidate.correct, strict=True)
    ):
        return False

    def fully_correct(decisions: CatalogDecisions) -> set[str]:
        return {
            song_id
            for song_id in decisions.expected_song_ids
            if all(
                match
                for match, expected in zip(
                    decisions.correct,
                    decisions.expected_song_ids,
                    strict=True,
                )
                if expected == song_id
            )
        }

    return fully_correct(candidate) > fully_correct(baseline)
