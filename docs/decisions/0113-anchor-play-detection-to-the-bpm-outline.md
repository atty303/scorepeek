# ADR 0113: Anchor PLAY detection to the BPM outline

## Status

Accepted

## Context

ADR 0112 assumed that the right-hand `GRAPH INFORMATION` band and `MAX SCORE` label had a fixed
position in both SP and DP. In SP, the graph panel can instead move beside the play lane according
to the player setting. That makes the screen-path layout v2 predicate setting-dependent.

The lower-center BPM panel is present in both SP and DP and does not follow the graph panel. Its
interior is not a stable anchor: loading leaves the values blank, fixed-BPM songs and variable-BPM
songs display different values, and variable-BPM presentation includes minimum and maximum values.
Color area alone is also insufficient because a RESULT background can contain enough cyan pixels.

Independent canonical measurements of the connected cyan BPM outline gave bounding boxes of
`339..365 x 70..71` pixels in retained SP PLAY frames and `363 x 70` pixels in the retained DP PLAY
frame. Qualifying components contained 4,516 through 6,431 pixels. Their first six rows contained
at least 286 pixels on one row, their middle rows contained at most 29 pixels on one row, and their
last seven rows contained at least 302 pixels on one row. SELECT and RESULT controls did not have
that connected hollow outline.

## Decision

- Screen-path layout v3 supersedes ADR 0112 only for PLAY presence. It searches the bounded
  lower-center ROI `(760,940,480,140)` for four-connected cyan components rather than reading the
  graph panel.
- A PLAY component must contain 4,000 through 7,000 cyan pixels, be 330 through 380 pixels wide and
  68 through 72 pixels high, have at least 280 cyan pixels on one of its first six rows, at most 64
  on every inspected middle row, and at least 300 on one of its last seven rows. These constraints
  encode the connected, wide, hollow panel outline rather than color area alone.
- BPM numbers, `MIN`, `MAX`, and all other interior text are ignored. Loading, fixed-BPM, and
  variable-BPM presentation therefore share the same screen predicate.
- The best matching component, or the largest component when none matches, supplies bounded raw
  geometry and row-profile diagnostics. The screen-path layout digest binds all thresholds.
- RESULT, MUSIC SELECT, MODE SELECT, and DECIDE TRANSITION remain peer predicates. Exactly one
  screen must match. The accepted result event, run-event, snapshot, and recognition-observation
  schemas do not change.

## Consequences

PLAY recognition no longer depends on the configured SP graph position or on note-lane geometry.
A cyan RESULT background cannot pass solely by exceeding a pixel threshold; it must also reproduce
the connected BPM-panel dimensions and hollow row profile inside the fixed search area. Existing SP
and DP corpus replay verifies the captured layouts. A target capture with the alternate SP graph
position remains required before claiming that configuration as live-verified.
