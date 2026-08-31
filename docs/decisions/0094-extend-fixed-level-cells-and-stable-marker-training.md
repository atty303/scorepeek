# ADR 0094: Extend fixed level cells and bind marker training to stable frames

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0091 for measured two-digit ANOTHER level layout; ADR 0092 for marker
  build-evaluation scope

## Context

The first seven reviewed sessions contained no two-digit ANOTHER level. Numeric character layout
v2 therefore defined two-digit cells only for HYPER and failed closed when the next reviewed
session supplied ANOTHER level 12. That session also exposed two independent truth problems around
`FlyAway`: the image displays FAST 156 rather than the initially authored 155, and the clear-type
label used display alias `H-CLEAR` where replay compares the resolved domain value `HARD CLEAR`.

The same session contained a non-stable result-exit frame after the previous-miss dash had faded.
Dataset rows carry episode truth outside the operator-selected stable sequence, so requiring every
source row to display the dash incorrectly treated transition pixels as supervised marker truth.

## Decision

- Advance the fixed character artifact to `scorepeek-result-numeric-character-layout-v3` without
  changing v2. Add measured equal-width 19-pixel ANOTHER two-digit cells at `(858,1038,19,19)` and
  `(877,1038,19,19)`. Runtime still performs no component detection or slot shifting.
- Normalize observed result `H-CLEAR` to domain value `HARD CLEAR`, just as `A-CLEAR` is normalized
  to `ASSIST CLEAR`. Raw OCR remains available in recognition evidence.
- Advance the private HOG/MLP build report to v2. Marker accuracy and build rejection use only
  operator-selected stable sequences. The report retains the count of all source marker rows as
  `source_total`; a non-stable blank or transition row is not relabeled as a dash.
- Correct private truth through create-only label and suite generations. Generated crops, complete
  labels, and model bytes remain outside the repository. The registered runtime manifest binds the
  final suite-derived dataset, layout v3, evaluation report v2, and deterministic model weight.

## Evidence

- The corrected active suite contains eight sessions and thirty-four episodes. Its numeric dataset
  contains 869 unique crops, including HYPER level 11 and ANOTHER level 12.
- Session-disjoint evaluation classifies all 423 stable numeric field observations exactly. The
  stable dash-marker evaluation classifies 86 of 86 rows; 199 total source rows remain recorded.
- Repeated final training produces model SHA-256
  `8bf99191ecde1c7c511f72ae676b75bdcd53f838a0a2d11321886f918ff1e127`.

## Consequences

- Level 11 and 12 can be observed under the measured HYPER and ANOTHER layouts, while level remains
  advisory and cannot veto an accepted song, chart, performance, or event.
- FlyAway no longer trains against a contradictory FAST label, and its display clear type resolves
  to the existing domain vocabulary.
- Unmeasured NORMAL, LEGGENDARIA, and two-digit BEGINNER level layouts still fail closed.
- Historical layout v2, build report v1, suites, datasets, and model bundles remain immutable and
  replayable.
