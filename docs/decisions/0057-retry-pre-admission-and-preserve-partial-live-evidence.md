# ADR 0057: Retry pre-admission and preserve partial live evidence

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0052 only for its at-most-once rule before session admission; ADR 0056 only for
  joining recognition observations to diagnostic predicate facts in a partial live session

## Context

INFINITAS startup can expose a Gamescope PipeWire source before its final stream is ready, and can
briefly expose more than one matching source. A target run observed one unique node, failed before
session admission, and then ignored the usable source because the pre-admission attempt had already
consumed that numeric node lifetime. Restarting scorepeek after startup admitted the same environment
immediately.

The resulting live capture and recognition components also showed that bounded diagnostic recording
can omit a predicate fact while retaining the corresponding recognition observation. Treating that
ordinary partial-recording condition as an unjoinable session prevented v3 publication even though
the recognition observation retained its own tick, monotonic timestamp, and scene.

## Decision

A unique source is consumed only after exact admission starts a session. Pre-admission acquisition,
negotiation, first-frame, or profile-admission failure returns to watcher state and retries after a
500 ms interval while the source remains unique. Absent and ambiguous candidate sets continue to
wait. Repeated pre-admission failure remains one low-cardinality watcher state and does not produce
stdout or repeated stderr. Once a session has started, its terminal failure still consumes that node
lifetime until removal is observed. A catalog unavailable, changed-binding, or transient load
failure before admission returns to the outer watcher loop so the active catalog is read again;
model, runtime, profile, diagnostic configuration, and worker startup failures remain
invocation-level errors.

During partial v3 publication, a retained recognition observation without a retained predicate fact
uses its own registered monotonic timestamp and scene as the join context. A missing or invalid
recognition context still fails publication. Complete sessions never use this fallback and reject a
recognition observation whose predicate fact is absent.

## Consequences

- Scorepeek can remain running through a transient or not-yet-ready Gamescope startup source.
- Persistent pre-admission mismatch is retried without a tight loop or repeated user output.
- Session failures do not turn into automatic same-lifetime reconnect loops.
- Bounded fact loss does not discard independently retained recognition evidence from a partial
  diagnostic.
- The full scorepeek-first startup sequence still requires a fresh target run with this revision.
