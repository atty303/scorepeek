# ADR 0139: Unify RESULT state and persist provisional plays

## Status

Accepted

## Context

Waiting for semantic RESULT close before publishing and saving a recognized result kept the overlay
and score views stale during the period in which the operator most needed feedback. The separate
`result_provisional_changed`, `result_detected`, and `result_ingest_changed` contracts also made one
attempt appear as several unrelated lifecycles.

## Decision

Use one `result_changed` payload with `inactive`, `provisional`, `retracted`, and `confirmed` states
through the diagnostic run event, public Event API, TUI, overlay, and score consumer. The stable
identity is capture session plus the existing play `attempt_id`; each transition keeps an independent
public envelope event ID and sequence. Provisional, retracted, and confirmed carry the same complete
result payload. Confirmed is terminal.

Publish inactive at admitted-session start and PLAY start. Publish provisional when the complete
result first resolves, retracted with the revoked payload when later evidence contradicts it, another
provisional after re-resolution, and confirmed at normal semantic RESULT finalization. Close-only
resolution publishes provisional immediately before confirmed. Session finish retains the last state.

Event API v2 uses `$XDG_RUNTIME_DIR/scorepeek/events.sock`, a v2 snapshot with one non-null result
slot, and v2 live events. It removes `result_ingest_changed` and does not dual-publish v1. The overlay
lamp is inactive for inactive, green for provisional/confirmed, and red for retracted. Values remain
SQLite-authoritative and refresh only after `score_store_changed` reports a successful transaction.

SQLite schema v2 upserts provisional updates and confirmation into one play keyed by session and
attempt. It fixes display time to the first provisional transition, deletes the play on retraction,
and recomputes the chart projection so every ordinary history/BEST/graph query immediately follows
the provisional state. A small attempt-origin table preserves the first timestamp across a
retraction/re-resolution cycle. Existing schema-v1 plays migrate as confirmed.

On database open, remaining provisional plays are promoted to confirmed with explicit recovery
provenance and without publishing synthetic events for an old capture session. This intentionally
prefers retaining a likely valid provisional play after an abnormal process end. Consequently, a
retraction whose SQLite write failed before a crash can later be recovered as confirmed; score-store
health exposes the failed write. A database-specific writer lock is held for the Store lifetime;
recovery begins only after acquiring it, so a concurrent process cannot promote a live writer's
provisional row.

## Consequences

Overlay and score views update while RESULT is visible. Recognition state no longer needs a second
ingest lifecycle or a result-local revision. Confirmed count/history remains monotonic and increments
only on confirmed. The public socket is stable across future protocol revisions but consumers must
still reject unsupported envelope schemas.

This supersedes ADR 0108's separate provisional/confirmed authority, ADR 0119's version-named socket
and v1 snapshot, ADR 0120's confirmed-only play insertion, ADR 0126's ingest lifecycle, and ADR 0130's
separate readiness event. ADR 0122 remains authoritative for the overlay consumer boundary except
where it described the older result lifecycle.
