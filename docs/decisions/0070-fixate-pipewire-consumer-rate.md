# ADR 0070: Fixate the PipeWire consumer at the recognition rate

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0056 only for offering an unconstrained PipeWire stream rate
- Complements: ADR 0027's common receiver and ADR 0056's 10 Hz recognition cadence

## Context

The Gamescope stream accepted a range from an unspecified rate through 240 fps while scorepeek only
consumed frames at 10 Hz. On the target host the negotiated scorepeek node reported `0/1` rate,
accumulated roughly one PipeWire error per source frame, and Gamescope repeatedly reported that its
consumer was out of buffers. Moving dequeue work outside the process event did not restore timely
buffer return.

The application has no use for an unconstrained delivery cadence. Retaining buffers until a later
poll also leaves the producer without a reliable hand-back point.

## Decision

Offer the PipeWire video stream at exactly 10/1 fps, matching the application sampling cadence. In
each process event, dequeue the bounded available set, immediately return superseded buffers, and
copy only the newest mapped frame into application-owned memory before returning the final buffer.
Keep the existing format, memory, size, and fail-closed frame validation contracts unchanged.

The source may render at 60 or 120 fps; scorepeek requests delivery at 10 fps. This decision does not
make source render rate a profile-acceptance condition and does not add conversion fallback.

## Consequences

- PipeWire buffer ownership is completed inside the event that announces available buffers.
- At most one source frame per 10 Hz process event incurs the 4K application copy.
- Intermediate source frames are intentionally not observable by recognition or diagnostics.
- Target-host validation must confirm the negotiated rate and absence of growing node errors and
  producer out-of-buffer warnings.

