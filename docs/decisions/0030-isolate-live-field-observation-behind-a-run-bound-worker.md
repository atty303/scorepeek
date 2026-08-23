# ADR 0030: Isolate live field observation behind a run-bound worker

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0025's immutable diagnostic run and ADR 0026's diagnostic worker

## Context

Live screen routing now yields opaque `LiveScreenRgb8Crops` whose pixels remain joined to an
admitted canonical frame. Model construction, catalog loading, preprocessing, inference, and song
resolution must not run in the PipeWire-facing capture loop. A model/catalog observer also needs a
different lifecycle from diagnostic recording: queue loss prevents that frame's field observation,
while diagnostic queue loss must not change recognition results.

The current registered PP-OCRv6 routines load model and dictionary bytes and construct an ONNX
session for each offline request. Reusing those functions directly in the capture loop would make
filesystem and model latency part of acquisition and would not prove that the loaded inputs match
the immutable recognition run.

## Decision

The application owns at most one production field-observer worker. Its loader runs synchronously
once, before worker and capture processing begin, and receives a strict session binding derived
from the complete diagnostic descriptor. The binding includes run ID, capture generation, capture
profile, normalizer, canonical layout, catalog, model, runtime, and their canonical identity. A
descriptor whose layout is not the current canonical layout is rejected before loading. A loader
failure prevents the worker from starting; there is no model or catalog fallback.

The worker accepts only an opaque `LiveScreenRgb8Crops` produced by `LiveRecognitionSession`.
Admission requires both the originating run ID and every immutable binding field to match the
worker. An equal digest tuple from another run is not interchangeable. The application transfers
the already-owned bounded crops through a `sync_channel` of capacity two with `try_send`; it never
waits for inference or queue capacity. A separate atomic permit count limits all accepted but
unconsumed results to two, including results already removed from the input queue. Queue full,
outstanding-result limit, binding mismatch, and worker loss are distinct typed offer outcomes.

The observer runs exclusively on the worker thread. Its output is wrapped by the worker with the
session binding, capture sequence, monotonic interval, and screen class; an observer cannot author
or replace that provenance. Each accepted offer returns a one-result pending handle. Dropping the
handle cannot block the worker. Consumption releases its permit; dropping it records abandonment.
Finish takes one race-free counter snapshot and also counts handles still unconsumed at that point
as abandoned. The observer output type remains application-defined so this execution boundary does
not invent interim field, song, suppression, or accepted-event schemas.

Finish queues after earlier observations and waits for at most five seconds by default. A timeout
is only a bounded caller-wait result: the thread may finish later, and a supervisor token prevents
another production observer from accumulating beside it. The token remains held through observer
and receiver destruction, including a blocking runtime destructor. Complete finish reports
submitted, completed, and abandoned counts. Worker disconnection is distinct from timeout.

The field worker does not own diagnostic storage, stdout, event delivery, model download, or remote
export. Diagnostic recording remains independently optional and non-interfering. Pixels, OCR text,
catalog strings, paths, environment strings, and arbitrary properties are not automatically added
to diagnostic facts or public output. No credential-bearing input exists at this boundary, so no
credential suppression or redaction classification is introduced.

## Consequences

- A production PP-OCRv6/catalog loader must still verify and construct its registered immutable
  inputs through this boundary; this ADR and its initial implementation do not perform live OCR.
- Application code must explicitly poll or wait for pending observations and decide how queue loss,
  observer errors, field stabilization, song decisions, suppression, and accepted events behave.
- A green synthetic worker test does not establish real-field correctness, inference throughput,
  capture support, or target-machine performance.
- Replay and diagnostics can later consume the same bound observer outputs, but neither may become
  an alternate recognition decision path.
