# ADR 0078: Emit catalog-constrained result domain events

## Status

Accepted.

## Context

The production result observer already accepted song identity and clear type, but difficulty,
level, notes, and current score were recorded as unimplemented placeholders. The active private
QOI suite contains fourteen visually reviewable result frames. Every current fixture is SP on the
1P side, while automatic play-mode, play-side, and savability detection remain later work.

The catalog is authoritative once a song identity is accepted. Chart OCR is auxiliary evidence:
using an OCR conflict to retract an independently accepted song would turn additional evidence into
a regression. Conversely, a catalog-unique chart can resolve a primary title tie without becoming
a vote against an already accepted primary result.

## Decision

- Observe all measured result and music-select text crops with the registered PP-OCRv6-small
  runtime. Recognition observation v6 records the raw text, typed result-field parsing, and chart
  resolution; retained v5 observations remain readable by offline evaluators.
- Correct the measured result layout so difficulty, level, notes, and the complete blue current
  score occupy separate regions. Current-score OCR first uses the complete field and performs one
  score-color-bounded retry only for an empty value or a trailing-zero ambiguity.
- Parse difficulty through the closed vocabulary with a unique one-edit allowance. Parse numeric
  fields fail-closed. Resolve the accepted song's SP chart from difficulty and notes, using a known
  level as an additional constraint. A missing level may be supplied by the unique catalog chart;
  a conflicting known level rejects the chart.
- Primary song acceptance is monotonic with respect to chart evidence. An accepted primary result
  is returned unchanged even if chart evidence is missing or contradictory. When the primary
  resolver is unknown only because its title margin or artist corroboration is insufficient, one
  catalog-unique SP chart may complete that decision if it identifies the selected primary
  candidate. It never rescues empty or out-of-bound title evidence.
- Emit `result_detected` once per result-screen episode after the existing two-observation/250 ms
  song and clear-type reducer is jointly stable and the current observation has one accepted chart
  plus a bounded current score. The typed `scorepeek-result-detected-v1` payload carries song ID,
  play type, difficulty, level, notes, score, and clear type.
- For the current corpus and runtime slice only, emit `savable=true`, `play_side=one_player`, and
  `play_mode=single_play` as explicit provisional contract values. Fixtures outside that admitted
  slice cannot generate this event until their detectors are implemented.
- Extend private regression labels to v2 with the visually reviewed result context. Publication is
  create-only; prior labels and suite generations remain immutable. Replay checks the full typed
  result context on all stable QOIs and reports all mismatches in one failure.

The event is published on the provisional run observation channel and retained run-event artifact.
This does not implement or advertise the accepted public `/v1.sock` API.

## Consequences

The existing active suite now exercises fourteen complete result contexts rather than only song and
clear type. Auxiliary evidence can prevent a complete domain event but cannot downgrade a known
song. SP/DP, 1P/2P, and savability detection remain explicit release blockers for widening the
runtime beyond the current admitted slice.

This supersedes ADR 0032 and ADR 0036 only for their result-field and selected-chart
`observer_not_implemented` contracts, and ADR 0059 only for leaving provisional result event
generation unimplemented.
