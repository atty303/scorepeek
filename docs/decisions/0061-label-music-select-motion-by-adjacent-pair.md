# ADR 0061: Label music-select motion by adjacent pair

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0060 only for treating a whole review span as one label unit; retains its
  unlabeled region-motion draft and context windows

## Context

ADR 0060 produced eleven review spans with 982 samples and 971 adjacent pairs. The spans are
screen-presence intervals plus 500 ms of context, not units of stable behavior: the longest span is
23.4 seconds and contains stopped selection, scrolling, and selection changes. Assigning one label
to a whole span would erase transitions and install false truth before evaluating dwell.

The current draft contains 838 adjacent pairs whose previous and current samples are both
`music_select`. The remaining 133 pairs touch predicate-unknown context. Those context pairs are
useful for visual review but are not music-select motion truth.

## Decision

Add a create-only offline application command:

```text
scorepeek-corpus music-select motion review-apply --output REVIEWED DRAFT DECISIONS
```

The canonical `scorepeek-private-music-select-motion-review-decisions-v1` document binds the exact
draft SHA-256 and contains bounded operator decision intervals. Each interval names one `span_id`,
an inclusive first and last current-sample sequence, and exactly one of `stationary`, `scrolling`,
or `selection_change`. Ranges are authoring compression only: application expands them to exact
adjacent pair identities. An interval that crosses a missing pair, a predicate-context pair,
another interval, or the selected draft fails closed.

The immutable `scorepeek-private-music-select-motion-reviewed-v1` artifact copies every adjacent
pair's exact timestamps, screens, source frame index, and separate list/active/central motion
evidence from the digest-bound draft. Eligible decided pairs carry `operator_reviewed`; eligible
omitted pairs remain `unknown/operator_review_required`; pairs touching a non-music-select sample
remain `unknown/screen_context` and cannot receive a decision.

Partial application is allowed so review can proceed without guessing. The artifact records
decision-interval, applied-pair, remaining-review-pair, and context-pair counts plus a `complete`
flag. Only `complete=true` can be considered a complete motion-truth input for a later evaluator.
Neither partial nor complete review chooses a motion threshold, adds dwell, changes runtime
recognition, or grants event authority.

The command is a synchronous deterministic transform of two bounded local documents. Its result
artifact, compact summary, create-only publication, and typed failure retain the complete operation
boundary; it does not add a separate diagnostic recording.

## Consequences

- Long screen-presence spans no longer force one false state over mixed behavior.
- Screen transitions remain visible without contaminating the motion-label denominator.
- Operators may review contiguous behavior as intervals while the stored truth remains pair-local.
- Dwell evaluation remains blocked until the 838 eligible pairs have complete reviewed coverage.
