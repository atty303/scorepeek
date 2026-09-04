# ADR 0119: Promote the public event socket

- Status: Accepted
- Date: 2026-09-04
- Supersedes: ADR 0058's observation socket and future accepted-event transport boundary;
  the roadmap's request-based `hello`/`get_status`/`subscribe` protocol and `music_select_detected`
  name. ADR 0108/0112/0114/0118 domain authority and interval rules remain unchanged.

## Context

Confirmed/provisional RESULT, current selection and supplemental SELECT best already have typed
publication points. The observation socket also exposed raw recognition and resolver diagnostics,
so renaming it would turn implementation details into public API. No product client in this
repository consumes the observation socket; the TUI and replay operate on internal state/events.

## Decision

Replace the observation socket with `v1.sock`, using a typed public projection and public snapshot
independent of run-event v11. Reuse the existing bounded Unix-stream delivery and socket ownership
rules. Keep internal events, recording and replay independent of socket publication.

Use connect → snapshot → live NDJSON rather than requests. Public events are confirmed RESULT,
provisional RESULT, current selection, supplemental best and operational status. Best invalidation
is represented by a null supplemental snapshot. Session lifecycle clears current state while the
latest confirmed result retains its original event identity and provenance.

Public sequence numbers count only public events. Snapshot and sequence boundary share the
publication lock; per-client boundaries filter older queued records. Queue overflow disconnects
existing clients, including when no subsequent record arrives. Slow/partially written clients are
isolated. Every event/snapshot is bounded to 1 MiB; an oversized public representation or worker
failure terminates public delivery for that invocation without altering recognition or acceptance.

This is explicitly a live API. Reconnection restores current state, not all missed plays.
Persistence, ACK/retransmission, history and UI implementation are outside this decision. The
wire reference and consumer fold rules live in [Event API v1](../event-api.md).

## Consequences

- There is one public endpoint, no old-name alias and no observation client migration in this repo.
- Raw diagnostics stay in the existing opt-in recording and internal TUI/replay paths. Diagnostic
  channel-health samples are additive fields in retained internal records; raw schema readers stay compatible.
- Public readiness/provenance comes from actual session admission and immutable bindings. A public
  event is not a claim that a capture profile has passed target accuracy or performance gates.
- Lossless history would require a separate approved persistence/recovery design.
