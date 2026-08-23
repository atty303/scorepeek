# ADR 0034: Score every catalog song without ranking

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0021's full-catalog search and ADR 0032's complete screen observations

## Context

The production field observer now emits every currently observed text field for one result or
music-select screen. Turning those strings directly into a nearest song, an accepted field, or a
single combined score would add policy before negative evidence, thresholds, chart observers, and
temporal agreement exist. Keeping only a top candidate would also discard ties and alternate field
evidence needed to evaluate that later policy.

The active catalog and open-text observations are already bounded and immutable within one
recognition run. A pure comparison layer can preserve the complete competitive domain without
changing the asynchronous session or diagnostic-recording contract.

## Decision

`CatalogCandidateDomain` precomputes text sequences from every song in the active catalog, ordered
by `ScorepeekSongId`. No song is removed because of INFINITAS availability, dictionary coverage,
observed distance, or a future acceptance threshold. Search-term title variants are excluded.

Each non-search title variant contributes its raw string and exact comparison key. Its bounded
ASCII/fullwidth folded key is included only when that key belongs to one song across the complete
title domain, preserving ADR 0019. Artist text uses the same raw, exact-key, and domain-unique
folded comparison forms as observation evidence; this does not make the folded form an accepted
artist identity. Raw and exact observation forms compare only with raw and exact candidate forms;
a folded observation form compares only with an admitted domain-unique folded candidate form, so a
cross-song folded collision cannot re-enter through the observation side.

An admitted catalog may contain a song whose only title variants are search terms. Because those
variants cannot compete, domain construction returns a typed error carrying that song ID instead of
panicking, silently dropping the song, or treating its search term as a title.

For each text field and every catalog song, the domain records both:

- minimum Levenshtein edit distance across the observation and candidate comparison forms; and
- maximum normalized Levenshtein similarity as exact integer `matching_units/compared_units`.

Distances operate on Unicode scalar values after the selected comparison-form transformation.
Absolute distance and normalized similarity are minimized or maximized independently; neither is
selected as the runtime policy. No floating-point score is stored.

Result candidate observations retain separate title and artist scores for every song. Music-select
candidate observations retain separate central-title, artist, and active-list-title scores for
every song. The two title presentations are consistency evidence for the same selection, not two
independent metadata votes. Explicitly unimplemented non-text fields remain part of the input
screen shape but create no fabricated score.

This layer does not rank, truncate, intersect, stabilize, accept, suppress, mutate selection
context, or emit an event. An empty catalog produces an explicit zero-candidate observation rather
than an unknown song reason. The function is synchronous, deterministic, filesystem-free, and has
no recording side effect; the application-owned live diagnostic run remains the owner when this
layer is later connected to the asynchronous field session.

## Consequences

- Later resolver experiments can compare exact, absolute-distance, normalized-distance, and
  cross-field policies without rerunning OCR or silently changing the competitive catalog.
- Candidate observations may be materially larger than a top-N result. Runtime cost and queue
  behavior must be measured before the Gamescope field-observation gate is considered complete.
- No candidate score in this layer is an accepted title, artist, song, result, or event.
