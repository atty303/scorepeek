# ADR 0072: Accept the Gamescope unspecified PipeWire rate

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0070 only for requiring an exact 10/1 PipeWire stream offer

## Context

The target Gamescope source offers BGRx 3840x2160 with an unspecified `0/1` framerate. The exact
10/1 consumer offer introduced by ADR 0070 has no intersection with that source while conversion is
disabled. In the first installed run, PipeWire rejected the link with `no more input formats (-22)`;
scorepeek remained `waiting_for_source`, created no session, and never reached buffer processing.

Recognition already owns an independent 10 Hz cadence. PipeWire delivery rate therefore need not be
fixed to implement application sampling, while timely dequeue and queue remain required to avoid
producer buffer starvation.

## Decision

Offer a framerate range with 10/1 as its preference and 0/1 through 240/1 as accepted values. Preserve
the negotiated producer value, including Gamescope's `0/1`, in the observed contract. Keep ADR 0070's
process-event behavior: dequeue the bounded available set, return superseded buffers immediately,
and copy only the newest mapped frame before returning its buffer.

Recognition and diagnostic admission continue to sample at the existing 10 Hz application cadence.
No format conversion, reconnect fallback, or source-rate profile gate is added.

## Consequences

- The target Gamescope source can negotiate without pretending that its unspecified rate is 10 fps.
- PipeWire delivery cadence and recognition cadence remain separate contracts.
- A fresh target run must establish both successful link negotiation and whether process-event
  buffer return stops scorepeek node errors and Gamescope out-of-buffer warnings.

