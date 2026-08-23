# ADR 0033: Own live field observation as one application session

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0030's run-bound worker and ADR 0032's complete screen outputs

## Context

The recognition session can route one admitted canonical frame to a complete screen-local crop set,
and the field worker can observe a matching crop owner. Leaving those two owners separate at the
application call site permits a caller to omit submission diagnostics, close the diagnostic run
before the observer, or accidentally pair a pending output with another run.

Field submission and inference remain asynchronous. Queue capacity loss, an unconsumed result, or
worker failure must remain distinguishable from screen inspection and from diagnostic-recorder
failure, without storing OCR text or making a song decision.

## Decision

`LiveFieldObservationSession` owns exactly one `LiveRecognitionSession` and one
`FieldObserverWorker` created from the same immutable descriptor. The observer loader completes
before the diagnostic-backed recognition run opens. The production constructor loads the exact
registered catalog, model, and runtime and constructs `RegisteredScreenFieldObserver`; there is no
resource or observer fallback.

For each admitted frame, the owner first performs the existing recognition inspection and canonical
diagnostic offer. An unknown screen returns `not_applicable` for field submission. A result or
music-select screen transfers its complete crop owner with non-blocking `try_observe`. Screen
observation, field-submission outcome, and diagnostic enqueue outcomes remain separate typed result
members. Binding mismatch, outstanding-result limit, queue full, or worker loss cannot replace the
screen observation.

The integrated pending handle is opaque and carries private owner and pending identities. Another
run rejects it before receiving or consuming its output. The owning session keeps an exact bounded
ledger of its at most two pending identities and source sequences. One pending handle yields at most
one bound field result. After consumption, later polls return `consumed`; they do not misclassify a
completed one-shot channel as worker loss. A disconnected handle reports worker loss once and is
terminal thereafter. A ready result is checked against the active run and recorded through ADR
0032's value-free `observe_fields` fact. Pending, consumed, and terminal states do not create facts.

Field offer loss and per-pending worker loss are recorded as field-specific diagnostic degradations
with the exact affected capture sequence. Finish closes the field worker first, records remaining
pending ledger entries as exact abandoned sequences, records lifecycle timeout or worker loss as an
unbound degradation rather than inventing a capture sequence, and only then finalizes the diagnostic
run. The generic worker finish outcome also snapshots the bounded abandonment count on timeout or
disconnection. Diagnostic opt-out, enqueue loss, or storage failure does not alter the screen or
field result.
OCR strings, runtime causes, pixels, catalog strings, paths, resource bodies, environment strings,
and arbitrary properties are not added to diagnostic records.

## Consequences

- Application code has one owner that preserves current-run submission, result provenance, and
  shutdown order without becoming a recognition decision layer.
- Synthetic conformance can verify complete output, opt-out non-interference, and capacity-loss
  diagnostics without fabricating live capture provenance.
- Gamescope capture-loop integration, real INFINITAS field observations, catalog candidate
  resolution, temporal agreement, acceptance, events, and target-host performance remain later
  gates.
