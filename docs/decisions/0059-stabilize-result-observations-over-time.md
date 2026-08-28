# ADR 0059: Stabilize provisional result observations over time

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0024 only for excluding result-local temporal state, and ADR 0038 only for
  excluding a provisional temporal result derived after frame-local resolution
- Complements: ADR 0056's 10 Hz diagnostic boundary and ADR 0058's separation of raw observations
  from terminal presentation

## Context

Ordinary `scorepeek run` presents every frame-local resolver decision. Retained result evidence
shows that this surface can flicker during scene entry or a single weak OCR frame even when the
surrounding decisions agree. Treating OCR strings or candidate metrics as interchangeable votes
would hide the resolver's fail-closed evidence contract and make a conflict look more certain than
either input.

The active private corpus has fourteen reviewed result episodes. Ten have sufficiently complete
ordered result observations for temporal analysis; four legacy episodes retain only sparse stable
observations and are excluded from temporal calibration. Across the ten analyzable episodes, 440
result observations contain 430 correct song acceptances, ten typed song unknowns, and no wrong
accepted song. Clear type contains 420 correct values, twenty unknowns, and no wrong values. Every
episode reaches the same correct song and clear type twice and three times consecutively. After the
first correct decision, one song observation returns to unknown for one tick and no clear type
returns to unknown. This evidence supports suppressing transient unknowns, but it does not support
majority voting, threshold relaxation, or release-accuracy claims.

Music-select observations do not yet have operator-reviewed stationary and scrolling intervals.
A dwell policy could therefore discard real selection changes or install a scrolling candidate
without an accuracy oracle.

## Decision

Add a synchronous deterministic result-temporal reducer after the existing frame-local result song
and clear-type resolvers. It never changes or replaces those raw observations.

- Song identity and clear type accumulate evidence independently.
- Two equal non-unknown observations within a maximum 250 ms observation gap stabilize a field.
- An explicit unknown clears pending evidence. Once stable, an unknown preserves the stable value
  within the same result-local interval.
- A different non-unknown value after stabilization is a typed conflict. It is not voted, averaged,
  or silently installed.
- A gap over 250 ms, reversed monotonic time, a different recognized screen, or a session boundary
  resets the applicable state. A missing observation never adds evidence.
- The reducer retains first and last source sequence, first and last monotonic time, evidence count,
  and required count. It has no wall clock, filesystem, queue, or model dependency.

`scorepeek run` emits `temporal_result_changed` only when this reducer changes state. The provisional
record uses the existing sequenced `scorepeek-run-event-v2` observation channel and carries typed
`pending`, `stable`, `conflict`, or reset state plus the bounded catalog presentation for a stable
song. The client snapshot retains the latest derived state. Raw `field_observation` records remain
unchanged and precede any transition they cause, so a consumer can replay or reject the derived
policy. A bounded raw `screen_changed` record is emitted when the synchronous screen predicate
changes; a non-result boundary precedes and resets any derived result state. Queue drop and client
failure retain ADR 0058's non-interference contract.

The TUI displays the stabilized result separately from raw OCR and frame-local catalog resolution.
Non-TTY stdout remains limited to watcher, session, and channel-health state changes. Recording
opt-out does not alter the reducer. Complete diagnostic observations remain the replay source; the
derived transition need not be duplicated into the recognition artifact because it is deterministic
from ordered raw decisions and monotonic timestamps.

This state is provisional recognition presentation, not an accepted domain event. It cannot assert
result-screen presence, savability, chart, score, play identity, deduplication, or capture support.
It is not sent to the future accepted `$XDG_RUNTIME_DIR/scorepeek/v1.sock` API.

Music-select stable-selection dwell remains unimplemented until immutable review labels distinguish
stationary stable spans, scrolling spans, and real selection-change boundaries. The existing
screen-local music-select resolver remains frame-local.

## Consequences

- A single result unknown no longer removes an already stabilized title, artist, song ID, or clear
  type from the interactive view.
- Two different accepted values become an observable conflict instead of presentation flicker or a
  majority decision.
- At the fixed 10 Hz cadence, normal stabilization adds about 100 ms after the first accepted
  observation. The 250 ms gap bound tolerates scheduling variation but does not itself contribute
  evidence.
- The ten analyzable result episodes support this initial policy, while the four sparse legacy
  episodes and unlabeled music-select runs remain explicit calibration gaps.
