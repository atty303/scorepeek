# ADR 0077: Require temporal selection before abandoning play

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0068 and ADR 0076 only for treating a raw music-select predicate as proof that an
  incomplete attempt returned to selection

## Context

The first target run after ADR 0076 preserved the selected song through decision and gameplay, but
lost it immediately before result. The complete joined diagnostic
`run-1788021301-919855565-1096112-session-1` recorded attempt 1 playing `Dreamship` at sequence
1491. After gameplay, sequence 2759 passed the independent music-select predicate while its OCR
fields read active title `5`, central title `RANDOM`, and no artist; song resolution was explicitly
unknown. The raw predicate immediately abandoned attempt 1 as `returned_to_select`. The real result
predicate arrived at 2764, and the same `Dreamship` identity stabilized at 2768, but it could only be
reported as an unlinked result.

ADR 0076 established that a frame-local music-select predicate is not selection-identity evidence,
but applied that rule only while the reducer was armed. The same predicate is also reachable during
the gameplay-to-result animation, so crossing the decision boundary does not make it authoritative.

## Decision

A raw music-select screen changes no play-attempt state. Unknown and raw music-select frames preserve
armed, decided, playing, result, and abandoned states alike.

Only a temporal music-select identity accepted as `stable` or `held_unknown` proves that an
incomplete decided or playing attempt returned to selection. On that update, the reducer first
publishes the incomplete attempt as `abandoned` with `returned_to_select`, then publishes the accepted
selection as `armed`. Pending, changing, empty, and unresolved music-select observations do not
abandon an active attempt. Existing temporal selection rules still clear or replace an armed
handoff.

The raw screen event continues to precede any derived state event. A real return publishes the
temporal music-select event before its ordered `abandoned` and `armed` play-attempt events. Event
schemas, predicate thresholds, OCR, temporal dwell, attempt IDs, and result authority do not change.

## Consequences

- `playing -> unknown -> music_select(unknown OCR) -> unknown -> result` keeps one attempt and lets a
  matching stable result confirm it.
- A true return to selection remains explicit, but only after temporal identity evidence rather than
  one color-predicate frame.
- The ordered event stream retains both the abandoned attempt and the newly armed selection instead
  of collapsing a real return into only its final state.
