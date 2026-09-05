# ADR 0126: Publish the result ingest lifecycle

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0119's closed list of public v1 events and status fields.

## Decision

The local public socket adds `result_ingest_changed` and the snapshot adds a nullable
`result_ingest` slot. The ingest has an opaque ID, `processing|persisted|failed` state, an optional
confirmed RESULT event ID and a bounded failure reason. This is the persistence lifecycle for one
RESULT screen episode, not another result or play event.

When score persistence is enabled, a RESULT semantic episode starts `processing`. A confirmed
`result_detected` attaches its event ID. The score worker's committed success (including an idempotent
duplicate) publishes `persisted`; a write error or five-second timeout publishes `failed` with
`persistence_failed`. A recognition failure publishes `recognition_failed`, and session termination
while processing publishes `interrupted`. A later completion cannot replace a failed state. The next
DECIDE transition or PLAY clears the slot. Merely returning to SELECT without a detected result does
not manufacture a failure. With scores disabled the slot remains absent.

`status` adds nullable `scores` and `recording` readiness. A consumer may ignore unknown event kinds
after validating the v1 envelope and sequence, which keeps additive local events forward-compatible.
The schema name remains v1 because compatibility with the earlier unpublished socket is explicitly
not retained for this milestone.

## Consequences

The overlay RESULT lamp can distinguish a usable persisted play from a recognition or persistence
failure without reading private diagnostics. Score and history widgets still query SQLite and never
derive committed state from this indicator. The event contains no database path, title, arbitrary
error text or diagnostic payload.
