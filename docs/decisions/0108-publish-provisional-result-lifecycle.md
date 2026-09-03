# ADR 0108: Publish a provisional RESULT lifecycle with the accepted payload

## Status

Accepted

## Context

RESULT identity and numeric performance can become jointly resolved before the semantic RESULT
episode closes. The accepted `result_detected` event deliberately waits for close-time field drain
and play-attempt confirmation, which is the correct persistence boundary but prevents the current
TUI and a future UI from presenting an already resolved score. Defining a second result payload
would duplicate every result field and allow provisional and confirmed contracts to drift.

## Decision

Run-event v8 adds `result_provisional_changed`. A `resolved` state carries the unchanged
`ResultDomainEvent` with `contract: scorepeek-result-detected-v2` and optional catalog
presentation. The payload describes result content only. The outer event kind supplies authority:
only `result_detected` is confirmed and eligible for score/history persistence.

A provisional result requires an accepted joint identity, two matching accepted numeric
observations, and an active RESULT attempt whose ID can populate the payload. Selection linkage,
observed gameplay, and final attempt confirmation remain close-time gates. A completely unlinked
RESULT has no attempt ID and emits no provisional result.

Each RESULT episode starts revision numbering at one. The reducer emits only the first resolved
state, a changed payload, a withdrawal, or a later re-resolution. Repeated identical payloads are
deduplicated. Losing identity or numeric acceptance withdraws with `evidence_unresolved`;
close-time rejection uses `attempt_rejected`; session termination uses `session_ended`. Successful
finalization emits confirmed `play_attempt_changed` followed by one `result_detected`, without a
withdrawal, and constructs both provisional and confirmed values through one payload builder.

The bounded diagnostic run-event artifact, headless replay, observation snapshot, and observation
socket retain the lifecycle. The debug socket and snapshot advance to v8. Readers accept run-event
v2 through v8 and reject unknown v9. The future public `/v1.sock` will route the typed lifecycle,
but production UI must not depend directly on the debug observation socket.

The TUI renames the pane to Latest result. An active provisional value is marked `PROVISIONAL` and
temporarily replaces the newest `CONFIRMED` value. Withdrawal restores the newest confirmed value.
Only `result_detected` increments confirmed count and history.

## Consequences

UI latency no longer depends on leaving RESULT, while score persistence and replay authority remain
unchanged. Diagnostic recording failure remains non-interfering. Consumers must select authority
from the outer event kind and must not infer it from the nested payload contract.

This partially supersedes ADR 0097 only for saying that screen-local evidence never produces a
domain-facing lifecycle event and that no cancellation event exists. It partially supersedes ADR
0084 and ADR 0088 only for accepted-only Latest-result presentation. Their confirmed-event
authority, persistence, and diagnostic separation remain in force.
