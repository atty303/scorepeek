# ADR 0081: Classify mode and music selection by fixed references

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0044 only for treating its aggregate predicate as sufficient to classify music selection

## Context

ADR 0062 retained operator-reviewed evidence that MODE SELECT sequences 898 through 907 passed all
three aggregate MUSIC SELECT predicates. The screens share the cyan header, colored level column,
bright fixed-label region, and most of their layout, so aggregate color counts cannot distinguish
them. Treating MODE SELECT as `unknown` would avoid field OCR but would also discard an observed,
stable screen boundary needed by diagnostics and the routine display.

Two operator-approved 410x60 RGB QOI crops independently taken from scorepeek captures are stored in
`crates/scorepeek/assets/screen-references-v1/`. Their SHA-256 digests are bound in the screen-path
layout: `ef3a27957b0c4999de6f5a8c7f240efac915a3f79bd4de1aefc2c518d148f1bb` for MUSIC SELECT and
`142f956367402a71738e557502c3b78c5f624731f1313c0a17508ff31e6f7ca7` for MODE SELECT. On retained
canonical frames, normalized grayscale cross-correlation scores were 1,000,000 versus 969,130 ppm
for MUSIC SELECT and 1,000,000 versus 969,436 ppm for MODE SELECT.

## Decision

Keep ADR 0044's three aggregate predicates as a cheap prerequisite. Only after they all pass,
compare both embedded references over canonical ROI `x=46, y=46, width=418, height=68` using
`imageproc` normalized grayscale cross-correlation. Decode the QOI references once per process.

Classify a reference winner only when its score is at least 900,000 ppm and exceeds the other score
by at least 20,000 ppm. A MUSIC SELECT winner produces `music_select`; a MODE SELECT winner produces
the new first-class `mode_select` screen. Failure to meet either condition remains `unknown`.
Version the algorithm identifier, search ROI, reference dimensions, digests, absolute threshold,
and winner margin in `screen-path-layout-v1.json`. Record both scores and both thresholds in every
screen-predicate diagnostic fact.

`mode_select` is screen context only. It emits screen-change observations and appears in diagnostics
and the TUI, but it has no field crop, performs no text OCR or catalog lookup, and supplies no
play-attempt transition evidence. Like any non-MUSIC SELECT screen boundary, it resets provisional
music-selection temporal state.

## Consequences

- The known MODE SELECT false positive is represented explicitly instead of entering the MUSIC
  SELECT field path or collapsing to `unknown`.
- Matching cost is paid only after the existing aggregate checks and is bounded to two references
  over a small fixed search region at the 10 Hz recognition cadence.
- Any reference or matching-contract change updates the layout binding and requires corpus replay.
- The retained corpus verifies the two known screens; target-host latency and broader screen
  coverage remain separate qualification evidence.
