# ADR 0130: Separate RESULT readiness from score-store invalidation

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0125's overlay RESULT lamp and SQLite readback triggers; ADR 0126's consequence that the overlay lamp represents result persistence.

## Decision

The status widget's RESULT lamp represents whether the current RESULT screen produced a usable
provisional result. A newly entered RESULT episode is unlit. A resolved provisional result turns it
green; withdrawal turns it red; resolution after withdrawal returns it to green. Leaving RESULT
without ever resolving turns it red. Green or red survives SELECT, DECIDE and PLAY, transient
UNKNOWN, and a public-socket reconnect in the same session. The next RESULT episode resets it to
unlit. Session finish, process stop, or a new capture session also clears it. The lamp has no amber
state and does not wait for SQLite persistence.

`result_ingest_changed` and its snapshot slot remain the public diagnostic contract for
`processing|persisted|failed` persistence state. They no longer drive overlay presentation.

The public live API adds transient `score_store_changed`. It contains the committed chart identity
(`scorepeek_song_id`, `play_type`, `difficulty`) and an invocation-local monotonic store revision,
but no score values or database path. The score worker returns chart identity with every completion.
After every successful transaction, including an idempotent duplicate or otherwise unchanged
write, the parent publishes this event. It has no snapshot slot because it is an invalidation hint,
not database state.

An overlay whose selected chart matches the event immediately rereads SQLite. Selection changes
also read the current committed state. A five-second poll remains only as recovery for external
database writers, a lost socket notification, or reconnect timing. `music_select_best_observed` and
`result_ingest_changed` never imply that their asynchronous transaction has committed.

The parent-owned overlay configuration controller must not write structured diagnostics directly to
the parent's terminal stream. It records them into a bounded in-process queue. The routine loop
drains that queue into the existing `overlay_observed` run-event/recording path without requesting a
TUI redraw. Queue overflow is summarized with a dropped count. Overlay child diagnostics retain
their existing piped collection, and child exit or failure remains an operator-visible warning.

## Consequences

The RESULT lamp is useful before leaving the result screen and remains visible long enough to notice
an unresolved attempt. Persistence can fail after a green lamp; `result_ingest_changed` and score
health remain the diagnostic surfaces for that different question.

SELECT best and RESULT writes can refresh the current overlay as soon as the committed transaction
is observable, while all displayed values continue to come from SQLite. Consumers that do not know
`score_store_changed` may ignore it under the additive v1 event rule and retain polling behavior.
Controller edit activity remains recordable without injecting JSON lines into the TUI output.
