# ADR 0075: Record reconstructable routine diagnostics

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0026 only for sharing one capacity-two queue between frames and facts
- Complements: ADR 0037, ADR 0056, ADR 0058, and ADR 0071

## Context

An operator observed an `armed` music selection lose its song before gameplay. The retained run
contained screen predicates and value-free field counts, but not the exact recognition output or
the temporal and play-attempt events that consumed it. The recognition component had also stopped
starting after its eight-generation store reached capacity. The run was therefore marked partial
without preserving the information needed to distinguish a resolver change, reducer transition,
or presentation defect.

The diagnostic writer used one capacity-two queue for QOI frames and small facts. QOI encoding and
filesystem publication can occupy that queue long enough to drop facts. ADR 0026 explicitly made
capacity two an initial memory policy rather than a supported performance threshold. Increasing
that shared message count would permit multiple batches of large canonical and source frames to
consume an excessive and poorly expressed amount of memory.

Recognition cadence also assigned a tick sequence before skipping a due tick while the field
worker was busy. The next processed frame then appeared to the diagnostic worker as a capture
sequence gap even though capture had not lost the frame.

## Decision

One ordinary recorded session retains enough ordered local evidence to reconstruct every
application-level recognition and reducer transition:

- the recognition observation stream retains the exact bounded OCR fields, catalog-bound resolver
  evidence, decision, reason, and selected or runner-up identity already required by ADR 0037;
- a complete event stream retains every `scorepeek-run-event-v2` record delivered to routine
  output, including raw field observations and generated temporal music-select, temporal result,
  armed, play-attempt, and session-boundary changes in publication order; and
- each due recognition tick skipped because a field observation is outstanding is recorded with
  its tick sequence and monotonic interval as `recognition_busy_skip`, not inferred as a capture
  gap.

The event stream is an operator-owned diagnostic artifact, not the accepted event API. It uses a
dedicated bounded non-blocking worker and a capacity-256 queue of already serialized records. Each
record remains limited to 1 MiB, the stream to 250,000 records and 512 MiB, and producer drops make
the component and joined session explicitly partial. A complete component fsyncs the stream and
publishes a digest-bound manifest. Recording failure never changes capture, recognition, socket
delivery, terminal output, or reducer state.

Canonical/source frame messages retain the capacity-two queue because its entries may own large
pixel buffers. Small diagnostic facts use a separate capacity-256 queue drained by the same
recorder owner. Finish drains both queues before publishing the manifest. This protects causal
facts from QOI head-of-line pressure without multiplying the large-frame memory bound.

Recognition and routine-event component stores reclaim the oldest inactive complete or partial
generation before admitting a new generation when their generation or aggregate byte reserve is
exhausted. The ordinary-run lock makes all entries inactive at admission. Joined diagnostic
sessions remain the retained operator artifact; component stores are publication staging and must
not disable a fresh run merely because old components remain.

## Consequences

- A later investigation can replay the exact reducer inputs and compare them with the exact state
  changes shown by the TUI and observation socket.
- Busy skips remain explicit missing recognition input; they cannot be mistaken for PipeWire or
  capture loss.
- QOI persistence can still degrade independently, but it cannot consume the queue slots reserved
  for causal facts or routine events.
- A session is complete only when capture facts, recognition observations, and routine events are
  all complete. Missing any component remains visible rather than silently publishing a
  reconstructable-looking session.
