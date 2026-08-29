# ADR 0064: Reject time-only music-select dwell

- Status: Accepted
- Date: 2026-08-29

## Context

The complete v2 motion review distinguishes 712 stationary, 84 scrolling, 30 selection-change,
12 operator-context, and 133 predicate-context adjacent pairs. A dwell duration cannot be selected
from those denominators alone: the runtime observes frame-local song resolution, not operator
motion truth. The evaluator must therefore replay the session-bound OCR strings through the exact
catalog generation and production music-select resolver before comparing temporal output with the
reviewed pairs.

## Decision

Add the create-only offline command:

```text
scorepeek-corpus music-select dwell evaluate --store ROOT --catalog-store ROOT --reviewed REVIEWED --output REPORT [--policy DWELL_MS ...]
```

The input must be a canonical, complete `scorepeek-private-music-select-motion-reviewed-v2` set
bound to the active corpus suite and session. The evaluator verifies the session's observation and
catalog bindings, loads that exact content-addressed catalog generation, and replays the retained
OCR strings through `scorepeek-music-select-active-prefix-corroborated-v1`. Each candidate policy
stabilizes only after the same accepted song ID persists for the requested milliseconds; unknown or
a different accepted ID clears or replaces the candidate. Operator motion labels never become a
runtime input. They classify the derived state after each adjacent pair.

The immutable `scorepeek-private-music-select-dwell-evaluation-v1` report keeps stationary,
scrolling, selection-change, operator-context, and predicate-context denominators separate. For
each policy it reports stationary-run coverage and stabilization latency, candidate replacements,
accepted and unknown observation applications, stability or first stabilization on nonstationary
pairs, and whether a prior stable ID reset at an operator-reviewed selection change. The report is
`offline_descriptive_only`, sets `runtime_policy_selected=false`, contains no OCR or catalog
strings, and grants no stable-selection or event authority.

Evaluate 100, 200, 300, and 500 ms by default. Against reviewed-set SHA-256
`aa59dc31a678c4db633db0391747642de49a48e466bf53421c2054f9c68b912e`, all four policies miss two
selection-change resets. Nonstationary stable-pair counts are respectively 24, 18, 17, and 16;
stationary-run coverage is 16/27, 16/27, 13/27, and 13/27. The 500 ms candidate therefore loses
coverage without eliminating false stability. The canonical evaluation SHA-256 is
`5c7954152b95ed6f14b58b7992643df62ef0879841997680fa59cb24318c8a8c`. Select no runtime policy.

## Consequences

- Increasing dwell time alone cannot make current music-select resolution safe for temporal
  acceptance.
- A later design needs an independently observable reset signal or a resolver change that clears
  stale accepted IDs at selection changes, followed by another offline comparison.
- The report does not identify the correct song within a stationary run because the motion review
  labels selection identity changes but does not assign song IDs. Stable-selection accuracy remains
  a separate labeled-evidence requirement.
- Exact catalog generations can be loaded read-only without changing the active manifest, keeping
  historical evaluation bound to the session decoder input.
