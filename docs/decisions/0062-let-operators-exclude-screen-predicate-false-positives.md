# ADR 0062: Let operators exclude screen-predicate false positives from motion truth

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0061 only for requiring every predicate-eligible pair to carry a motion state

## Context

ADR 0061 treated every adjacent pair whose two retained predicates were `music_select` as eligible
for exactly one of `stationary`, `scrolling`, or `selection_change`. Visual review of the bound
video disproved that premise. Sequences 898 through 907 show the separate MODE SELECT screen, yet
the retained production predicate classified every frame as `music_select`. At sequence 898 the
three recorded presence counts all passed their configured thresholds: 8,740/7,000 cyan-header
pixels, 26,743/1,000 colored-level pixels, and 4,892/4,000 bright-label pixels. The review draft's
packet PTS, decoded-frame PTS, and observation timestamp already agree, so this is not a video
binding or seeking error.

Assigning a motion state to those frames would convert a known screen-predicate false positive into
false dwell truth. Tightening the production predicate from one observed negative is a separate
calibration decision and is not justified by this review.

## Decision

Replace the review-decision and reviewed-set schemas with v2. A v2 decision interval may use
`screen_context` in addition to `stationary`, `scrolling`, and `selection_change`.
`screen_context` is accepted only for a pair whose two retained predicates are `music_select`; it
records `unknown/operator_screen_context` rather than a motion state. Pairs already touching a
non-music-select predicate remain `unknown/predicate_screen_context` and cannot receive a decision.

The v2 completeness record separates:

- operator-reviewed motion pairs;
- operator-excluded screen-context pairs;
- predicate-derived screen-context pairs; and
- predicate-eligible pairs still requiring review.

`complete=true` requires only that no predicate-eligible pair remains unreviewed. Operator-excluded
pairs never enter a later motion or dwell denominator. The original digest-bound interval,
overlap, bounds, canonical encoding, and create-only publication contracts remain unchanged.

Applying the first observed exclusion, sequences 899 through 907 in span 0001, produces nine
operator-context pairs, 133 predicate-context pairs, and 829 pairs still requiring review. This is
partial review evidence only; it neither changes the production screen predicate nor selects a
motion threshold or dwell policy.

The transform remains short, synchronous, deterministic, and safely repeatable with a new
create-only output path. Its typed failure, compact result summary, and immutable result artifact
identify the complete operation boundary, so no separate diagnostic recording is added.

## Consequences

- Human review can fail closed when the retained screen predicate is visibly wrong.
- Motion truth no longer silently inherits screen-predicate false positives.
- Predicate calibration can use the excluded pairs as measured evidence without being changed by
  the review-application command.
- Existing v1 empty or partial review documents must be recreated as v2; no complete v1 reviewed
  set exists.
