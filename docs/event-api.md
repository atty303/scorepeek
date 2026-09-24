# Event API v5

The core supplies typed domain decisions. `scorepeek-runtime` projects their
public contract names and derives RESULT `play_mode` from `play_type`. The
MUSIC SELECT best observation ID, revision, and layout are determined by core.

`scorepeek run` owns the stable Unix socket `$XDG_RUNTIME_DIR/scorepeek/events.sock`. A connection
receives one UTF-8 NDJSON `scorepeek-event-snapshot-v5` record followed by
`scorepeek-event-v5` records. There is no request, handshake, subscription message, ACK, retained
event log, or second version-named socket.

## Envelope and identity

Every live record contains `schema`, `invocation_id`, a gap-detecting public `sequence`, unique
`event_id`, `emitted_unix_ms`, `emitted_monotonic_ms`, nullable `capture`, and `event`. Capture
context contains only the admitted, nonempty `session_id`. A null `capture` means no capture
session owns the event. Source and resource details belong to the runtime diagnostic stream.
The result attempt identity is `(capture.session_id, state.result.attempt_id)`; no separate result ID
or result-local revision exists. Every state transition has its own envelope event ID and sequence.

## Events and authority

| `event` | Additional fields and meaning |
| --- | --- |
| `game_version_changed` | `source_sequence`, `version`. Emitted once, only when three equal structurally valid TITLE OCR observations identify the current capture session's complete 20-character version. |
| `result_changed` | `source_sequence`, `state`. The state is `inactive`, `provisional`, `retracted`, or `confirmed`. Provisional, retracted, and confirmed states carry the same complete `song` and `scorepeek-result-detected-v4` `result` payload; retracted also carries a bounded `reason`. For SP and DP, `result.play_side` is the string `one_player` or `two_player`. |
| `music_selection_changed` | `screen_episode_id`, `source_sequence`, `revision`, `state`. Current chart presentation plus required string `play_side` for both SP and DP. |
| `music_select_best_observed` | Nullable supplemental SELECT-best snapshot. It is not a play. |
| `screen_state_changed` | Nullable semantic screen presentation state. |
| `status_changed` | Current watcher, capture, dependency, recording, and score-store readiness. |
| `score_store_changed` | Live-only chart invalidation after every successful relevant SQLite transaction, including unchanged/idempotent transitions. It contains no score values. |

The result transition is explicit. Capture-session start and PLAY start publish `inactive`.
Recognition publishes `provisional` as soon as the complete result resolves; contradictory evidence
must stabilize across two fresh matching observations before it publishes `retracted` with the
revoked payload; re-resolution publishes another `provisional`. A single contradictory complete
observation remains a challenger, and normal semantic RESULT finalization confirms the previously
stable payload. If the result resolves only during close-time
drain, provisional is published immediately before confirmed. Confirmed is terminal for that attempt.
Session finish retains the last result state; the next admitted session publishes inactive.

Only confirmed increments the process TUI's confirmed count/history. The overlay lamp maps inactive
to off, provisional and confirmed to green, and retracted to red. Overlay values remain SQLite query
results, not direct rendering of the result event payload.

## Snapshot and consumer state

The snapshot contains `schema`, `invocation_id`, `next_sequence`, `status`, one non-null `result`
record, nullable `music_selection`, `music_select_best`, and `screen_state` records, and nullable
`game_version`. `game_version` is `null` at capture-session start, becomes the exact complete screen
string after `game_version_changed`, and returns to `null` when that session ends. Before the
first live transition, `result` is a synthetic sequence-zero inactive record. Replace each slot with
its corresponding live event and require every live `sequence` to equal `next_sequence`. Reconnect
and replace local state after a disconnect or gap. `score_store_changed` has no snapshot slot; reread
the named chart from SQLite.

Unknown additive v5 event kinds and fields may be ignored after envelope and sequence validation.
Consumers must reject unknown schema versions. A process restart creates a new invocation and loses
socket-only state/history; SQLite is the durable play-history authority.

## Delivery and diagnostics

Delivery remains bounded and live-only: at most eight clients, a nonblocking 64-record producer
queue, and at most 1 MiB per event/snapshot including newline. A slow/partial client is disconnected;
queue overflow invalidates existing streams; encoding or socket-worker failure disables public
delivery without stopping recognition or the independent score consumer. The runtime replaces only
a stale socket and removes only the inode it owns.

Raw OCR, version candidates and failures, resolver scores, processing timings, paths, and history arrays stay outside the
public API. They are recorded separately in the private runtime diagnostic stream described in
[runtime diagnostics](diagnostics.md). `diagnostics.sock` does not alter this socket's schema,
snapshot, queue, reconnect, or delivery contract.

## SQLite interaction

Provisional and confirmed states upsert one play row keyed by session and attempt; a replacement does
not change its first-provisional display timestamp. Retraction deletes that play immediately and
recomputes affected RESULT/previous-best facts, so history, BEST, and graph reads no longer include
it. Confirmation updates the same row. The database stores one
`scorepeek-stored-result-v2` projection instead of retaining a public event envelope. Each play row
also stores `play_side` as a required queryable column; chart-best identity and aggregation remain
`(song_id, play_type, difficulty)` and do not split by side. Opening a version-three database
atomically migrates it to version four: stored SP sides are preserved and legacy DP rows are assigned
`one_player`, while each stored-result document is rewritten to the current contract. On database
open, an unclosed provisional row is promoted to confirmed with
recovery provenance but no synthetic old-session socket event. A database-specific lifetime lock
admits only one score writer, so another live writer's provisional row cannot be mistaken for crash
residue. Persistence failure is represented by score-store health/status; the removed
`result_ingest_changed` lifecycle has no replacement. The capture-session game version is socket
and recording-manifest state only; it is not stored in the SQLite score history.
