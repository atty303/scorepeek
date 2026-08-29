# Offline temporal evaluation

`scorepeek-corpus temporal evaluate` compares registered result-local temporal policies against the
ordered recognition observations in the active, operator-reviewed private corpus suite. It is a
read-only descriptive evaluator: it does not alter the suite, select thresholds, accept events, or
grant release-accuracy authority.

```text
scorepeek-corpus temporal evaluate --store /absolute/private-corpus-v1 --policy 2:250 --policy 3:250
```

`--policy` is `REQUIRED_OBSERVATIONS:MAXIMUM_GAP_MS`. Omitting it compares the runtime policy
`2:250` with `3:250`. One to sixteen distinct policies are accepted; observations are bounded to
2–16 and the gap to 1–60,000 ms. `mise run corpus:temporal:evaluate` selects the same default private
corpus root as `mise run corpus:test`.

## Episode binding

The evaluator verifies the active suite, session, label, and observation-object digests before
reading the bounded NDJSON stream. The label must bind that suite entry's session, have `include`
disposition, and reference stable sequences present in the session's canonical-frame set. A labeled
stable sequence is assigned to the raw result interval that contains it, including the sequence
range between a retained result observation and the next non-result boundary. Multiple labels may
not claim one interval. An interval with fewer than two retained result observations is reported as
`insufficient_temporal_observations`; a label that cannot be assigned or has multiple possible
assignments is excluded with a separate typed reason.

Current `scorepeek-recognition-observation-v6` and legacy v5 shapes are decoded into the same typed
input. Only an accepted song resolution supplies a song ID. Unknown resolution remains unknown,
and clear type uses the production resolver rather than an evaluator-specific mapping. A merged
stream's predicate-only `fields: null, song_id: null` placeholder contributes a screen boundary but
not a reducer observation, matching the production path's missing-evidence behavior.

## Report semantics

The stdout document has schema `scorepeek-private-temporal-evaluation-v1` and authority
`offline_descriptive_only`. It contains no OCR or catalog strings. Its aggregates mean:

- `raw_observations` counts each analyzable result observation as correct, incorrect, or unknown for
  song and clear type before temporal reduction.
- Each `policies` entry runs the production `ResultTemporalReducer` independently for every episode
  and reports the final song and clear-type state as stable correct, stable incorrect, conflict, or
  unresolved.
- `joint_stable_correct` requires both final field states to equal the operator label.
- stabilization latency starts at the first retained result observation and ends when both fields
  first become stable and correct. Observation-count and millisecond distributions use nearest-rank
  p50 and p95 over final jointly-correct episodes.
- transition counts expose gap resets, conflicts, and pending-candidate replacements. Per-episode
  results retain opaque episode IDs and sequence bounds for local review.

Comparisons are meaningful only for the active suite generation printed in the report. A policy
with better retained-corpus coverage is not automatically safer: a wrong-accept challenge set,
title-disjoint holdout, and calibrated false-accept denominator remain separate promotion gates.
