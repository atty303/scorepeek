# ADR 0026: Isolate diagnostic I/O behind a bounded application worker

- Status: Accepted
- Date: 2026-08-22
- Complements: ADR 0025's synchronous diagnostic run writer

## Context

QOI encoding, create-only publication, and fsync are intentionally synchronous
inside the strict writer. Calling that writer from capture or recognition would
make diagnostic storage latency part of game-session behavior. An offline replay
also needs to exercise the same queue and writer without making recognition
success the evidence trigger.

## Decision

The scorepeek application owns one single-producer diagnostic worker handle per
run. The worker thread exclusively owns the synchronous recorder. The normal
producer transfers an already-owned canonical RGB8 frame through a Rust bounded
`sync_channel` of capacity two using `try_send`. The producer applies the fixed
sampling cadence before queue admission, while the writer repeats the check as
a validation backstop; dense capture input therefore cannot occupy the queue
with frames that are ineligible for recording. The producer never waits for QOI
encoding, filesystem publication, or queue capacity. A full queue drops only diagnostic
evidence and returns `queue_full` to the diagnostic caller. It does not alter
recognition, event delivery, stdout, exit status, or other application state.

The producer retains at most 4,096 reason-and-sequence queue-drop entries. Any
additional drops remain visible as bounded reason counts and explicit log
truncation in the completion manifest. A disconnected worker is distinct from a
full queue. Queue, worker, writer, and completion errors remain diagnostic error
types and are recorded once at their owning boundary.

The producer observes every offered sequence and timestamp before cadence
filtering. It records real capture gaps and invalid ordering in the bounded
ledger. The writer therefore validates sampled-frame ordering but does not infer
that capture sequences omitted by the cadence gate are missing evidence.

Run completion first queues all bounded drop evidence and then a finish message.
Enqueue and response waiting together are bounded to five seconds. A timeout
sets cooperative cancellation and returns `flush_timeout` without doing more
filesystem I/O on the caller thread. Because a Rust thread cannot safely
interrupt a blocked filesystem call, timeout is not a terminal run-state claim:
an operation already past its cancellation check may publish a valid manifest
later. The manifest, when present and valid, is the authoritative eventual
terminal record. The application permits only one production worker; while a
timed-out worker remains alive, a later run fails as `worker_unavailable`
instead of accumulating threads, buffers, or descriptors. `run.json` without a
valid completion manifest is observable partial evidence at that observation
time. Opt-out starts no thread and writes no files.

The create-only offline replay driver uses the same worker and writer. Unlike the
live producer, it may retry a full queue only until the same five-second bound so
offline producer speed does not manufacture a live queue drop. Its strict JSON
request binds its exact bytes, run ID, monotonic boundary, immutable runtime and
recognition inputs, extraction digest, and ordered frame IDs/times. Canonical
JSON serialization is not required because the exact supplied bytes are the
digest-bound artifact. Every frame
is loaded through `CanonicalFrame::read_extraction`; capture-profile,
normalizer, and extraction bindings must agree before enqueue. The requested
evidence sequence must already be sparse at the fixed sampling interval; the
digest-bound extraction may contain additional valid frames that are not
offered to this run. The request frame IDs must be unique, advance in
extraction decode order, and use the exact non-negative source PTS from the
extraction's fixed 1/1,000 time base. The
request and extraction digests are retained in `run.json`, while paths and
pixels remain out of the public JSON summary. Only a complete run with a final
manifest returns a successful replay exit status.

Replay does not run recognition, infer a game timeline, create mode/attempt/play
state, or establish live performance and capture support. It proves only that
strict canonical evidence can traverse the application queue and diagnostic
writer independently of recognition triggers.

## Consequences

- The live integration can transfer frame ownership without a second RGB copy,
  but that ownership boundary and target-host cost still require Bazzite
  validation.
- Queue capacity two is an initial bounded memory policy, not a supported
  performance threshold.
- Aggregate retention, status/list/freeze/delete/export controls, worker health
  persistence when no run can start, and crash recovery remain later slices.
- A safely interruptible process boundary would be required before claiming a
  hard terminal state at the five-second timeout; this thread-based slice claims
  only bounded caller waiting and bounded residual worker count.
- The accepted ordinary-session recording still requires a strict sparse
  canonical extraction before the new replay control can execute its complete
  0–458,300 ms timeline; the operator-reviewed label alone is not pixel evidence.
