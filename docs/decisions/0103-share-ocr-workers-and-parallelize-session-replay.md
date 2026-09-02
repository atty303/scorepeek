# ADR 0103: Share OCR workers and parallelize session replay

## Status

Accepted

## Context

Canonical replay previously owned one text pool per session and effectively serialized FFmpeg
decode. A large decoder stdout can apply pipe backpressure, so a single global decoder does not
provide useful session concurrency. The earlier offline policy also created one OCR session for
nearly every logical CPU; on the development host that reduced summed text latency while making
whole-corpus wall time worse.

Field jobs within and across frames are independent until their results enter the session timeline.
Timeline, selection, attempt, RESULT-finalization, and event state are session-local and ordered.

## Decision

Live and replay use the same registered field scheduler and single-threaded ONNX sessions. Live
uses `min(12, max(1, available_parallelism / 2))` text workers and retains two admitted field
frames. Replay creates one global text pool using
`min(12, max(1, available_parallelism - 4))` workers unless the operator supplies
`--text-workers N`. Pool dispatch uses one global round-robin cursor. Completion may be out of
order, but each session commits admitted frames by source sequence and drains them before semantic
episode finalization.

Replay initially queues only suite index and object digests. Complete session and label documents
are loaded when that session becomes active. Decoder concurrency is
`min(session_count, max(1, available_parallelism / 4))`, further constrained by the global memory
account; at most twice that many session runtimes are active so a completed segment can yield its
slot to an already-ready different session. Each scheduled step decodes exactly one segment, then
returns its session-local ordered state to the FIFO. Segments within one session remain ordered,
while different sessions decode concurrently and a multi-segment session cannot retain a decoder
slot across child reap or close-time drain. Every child uses `-threads 1`, a dedicated bounded
stdout reader, bounded stderr, timeout, kill, and reap. A blocked child backpressures only its
session. Close-time field drain runs in a separate bounded finalizer pool after child reap, so a
slow finalization cannot consume a decoder permit. A session-local failure performs bounded
recognition teardown before its memory reservation is released. Invalid leading entries are
reported in corpus order while later valid sessions still run.

There is no fixed session-count limit. Queued session metadata remains a bounded digest projection;
decoder reservations, active session state, and pending field frames consume the replay memory
account. `--memory-mib N` accepts 256 through 8192 MiB and defaults to 2048 MiB. Capacity pressure
reduces active decoders, active sessions, and pending admission by backpressure; offline replay
never drops a frame or field.

Recognition observations advance to v18 and retain the raw stage durations plus the text worker IDs
used by each frame. Its end-to-end wall clock keeps the inspection-start origin through ordered OCR
commit and live resolver/output completion. Replay summary v3 reports selected worker/decoder
counts, bounded active sessions, actual maximum decoder overlap, child count, tracked memory
high-water, memory/decoder/ordered-commit waits, stable per-session wall time, and whole-corpus wall
time.

## Consequences

Four sessions on a 24-logical-CPU host can run four independent FFmpeg children and feed one
twelve-worker OCR pool. Adding sessions increases queued digest metadata rather than creating
unbounded ONNX pools. Segment-level FIFO handoff prevents a slow multi-segment session from owning
a slot between children; a failed session does not stop the remaining queue, failures and final
reports are restored to corpus order.

The 20-second 1,061-frame goal and four-session throughput gate remain measured target properties,
not consequences of the topology. One-worker and default-pool replay must continue to emit equal
domain events. This supersedes ADR 0101's offline all-but-one worker policy and single-session outer
pipeline description; it does not change canonical retention, label truth, or public domain-event
authority.
