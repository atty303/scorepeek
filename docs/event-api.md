# Event API v2

`scorepeek run` owns the stable Unix socket `$XDG_RUNTIME_DIR/scorepeek/events.sock`. A connection
receives one UTF-8 NDJSON `scorepeek-event-snapshot-v2` record followed by
`scorepeek-event-v2` records. There is no request, handshake, subscription message, ACK, retained
event log, or second version-named socket.

## Envelope and identity

Every live record contains `schema`, `invocation_id`, a gap-detecting public `sequence`, unique
`event_id`, `emitted_unix_ms`, `emitted_monotonic_ms`, nullable `capture`, and `event`. Capture
context contains the admitted `session_id`, `capture_generation`, and immutable binding digests.
The result attempt identity is `(capture.session_id, state.result.attempt_id)`; no separate result ID
or result-local revision exists. Every state transition has its own envelope event ID and sequence.

## Events and authority

| `event` | Additional fields and meaning |
| --- | --- |
| `result_changed` | `source_sequence`, `state`. The state is `inactive`, `provisional`, `retracted`, or `confirmed`. Provisional, retracted, and confirmed states carry the same complete `song` and `scorepeek-result-detected-v2` `result` payload; retracted also carries a bounded `reason`. |
| `music_selection_changed` | `screen_episode_id`, `source_sequence`, `revision`, `state`. Current chart presentation plus the stable MUSIC SELECT `play_side` (`one_player` or `two_player`). |
| `music_select_best_observed` | Nullable supplemental SELECT-best snapshot. It is not a play. |
| `screen_state_changed` | Nullable semantic screen presentation state. |
| `status_changed` | Current watcher, capture, dependency, recording, and score-store readiness. |
| `score_store_changed` | Live-only chart invalidation after every successful relevant SQLite transaction, including unchanged/idempotent transitions. It contains no score values. |

The result transition is explicit. Capture-session start and PLAY start publish `inactive`.
Recognition publishes `provisional` as soon as the complete result resolves; contradictory evidence
publishes `retracted` with the revoked payload; re-resolution publishes another `provisional`; normal
semantic RESULT finalization publishes `confirmed`. If the result resolves only during close-time
drain, provisional is published immediately before confirmed. Confirmed is terminal for that attempt.
Session finish retains the last result state; the next admitted session publishes inactive.

Only confirmed increments the process TUI's confirmed count/history. The overlay lamp maps inactive
to off, provisional and confirmed to green, and retracted to red. Overlay values remain SQLite query
results, not direct rendering of the result event payload.

## Snapshot and consumer state

The snapshot contains `schema`, `invocation_id`, `next_sequence`, `status`, one non-null `result`
record, and nullable `music_selection`, `music_select_best`, and `screen_state` records. Before the
first live transition, `result` is a synthetic sequence-zero inactive record. Replace each slot with
its corresponding live event and require every live `sequence` to equal `next_sequence`. Reconnect
and replace local state after a disconnect or gap. `score_store_changed` has no snapshot slot; reread
the named chart from SQLite.

Unknown additive v2 event kinds and fields may be ignored after envelope and sequence validation.
Consumers must reject unknown schema versions. A process restart creates a new invocation and loses
socket-only state/history; SQLite is the durable play-history authority.

## Delivery and diagnostics

Delivery remains bounded and live-only: at most eight clients, a nonblocking 64-record producer
queue, and at most 1 MiB per event/snapshot including newline. A slow/partial client is disconnected;
queue overflow invalidates existing streams; encoding or socket-worker failure disables public
delivery without stopping recognition or the independent score consumer. The runtime replaces only
a stale socket and removes only the inode it owns.

Raw OCR, candidates, resolver scores, processing timings, paths, and history arrays stay outside the
public API. They are recorded separately in the private runtime diagnostic stream described in
[runtime diagnostics](diagnostics.md). `diagnostics.sock` does not alter this socket's schema,
snapshot, queue, reconnect, or delivery contract.

## SQLite interaction

Provisional and confirmed states upsert one play row keyed by session and attempt; a replacement does
not change its first-provisional display timestamp. Retraction deletes that play immediately and
recomputes affected RESULT/previous-best facts, so history, BEST, and graph reads no longer include
it. Confirmation updates the same row. The database stores one
`scorepeek-stored-result-v1` projection instead of retaining a public event envelope. On database
open, an unclosed provisional row is promoted to confirmed with
recovery provenance but no synthetic old-session socket event. A database-specific lifetime lock
admits only one score writer, so another live writer's provisional row cannot be mistaken for crash
residue. Persistence failure is represented by score-store health/status; the removed
`result_ingest_changed` lifecycle has no v2 replacement.
