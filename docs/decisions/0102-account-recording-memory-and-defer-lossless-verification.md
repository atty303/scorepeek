# ADR 0102: Account recording memory and defer lossless verification

## Status

Accepted

## Context

The first canonical recorder used a two-frame input queue and closed a segment at every intentional
timeline gap. Segment close synchronously waited for FFmpeg, hashed the output, decoded it back to
RGB24, and verified the decoded digest before accepting more frames. A normal long session therefore
turned one brief encoder stall into queue drops, then into many additional gap-driven closes and
further drops. The operator could finish a valuable play session before learning that its joined
recording was not importable.

QOI frames from the older foreground diagnostic retention also duplicated canonical pixels during
routine recording. The strict replay admission boundary belongs at corpus import, where a complete
session is already immutable and realtime capture is no longer at risk.

## Decision

`scorepeek run --record` uses one shared recording-memory account with a default soft limit of
1024 MiB. `--record-memory-mib MIB` changes that limit for the invocation and is invalid without
`--record`. The recorder input channel is logically unbounded: it has no independently selected
frame-count capacity and no fixed partition between queues, the transition ring, tick metadata, or
FFmpeg stderr. Those live recording allocations charge the shared account and expose current,
limit, and high-water byte counts. Per-frame admission charges the retained pixel owner plus channel
and message overhead; the fixed transition/tick metadata and bounded stderr storage are reserved
from the same account. Tick records are streamed directly to their create-new NDJSON artifact so a
long session does not accumulate an untracked in-memory index.

An allocation that would cross the limit is rejected rather than blocking recognition. Recording
health becomes sticky `degraded`, the affected pixel is omitted, and domain classification,
attempt finalization, and event emission continue. Later allocations may resume after accounted
memory is released, preserving as much diagnostic evidence as possible, but the session remains
partial. `pressured` begins at 75 percent of the shared limit. The TUI receives health changes
immediately and a current usage refresh at least once per second.

Intentional elision no longer closes the encoder. One segment contains up to 600 retained frames
in original retained order even when their source sequences have gaps. Chronology reset and session
end still close the active segment. Because label v5 identifies frames by sequence alone, a
chronology reset also makes the session partial instead of publishing an unusable complete replay.
The tick index is the authority mapping encoded frame order to original sequence and monotonic time.

The realtime recorder records the input RGB24 digest and frame count, waits for and reaps the
encoder, and records the encoded-file digest. It does not decode its own output. Canonical recording
v2 declares integrity verification `deferred_to_import`. Corpus diagnostic verification/import
decodes every segment, checks its frame count and RGB24 digest, and rejects corruption before a
session can enter an active suite.
The import decoder has a bounded two-minute segment deadline, continuously drains bounded stderr,
and kills and reaps FFmpeg on timeout, truncated output, or replay-observer failure.
Downstream replay and numeric-dataset authoring use the same segment/tick iterator; no consumer may
reopen a segment-backed frame identity as if it were a standalone QOI object.
Import also requires the tick index in the joined artifact inventory and rejects any sequence or
monotonic chronology reversal before publishing corpus objects.
Canonical v2 readers require the deferred-integrity mode and validate memory limit/high-water
invariants. Tick digesting and line parsing are streaming and line-bounded rather than whole-file
allocations.

Routine capture diagnostics retain facts only; they do not materialize legacy QOI or source-QOI
files beside the canonical segment. Explicit diagnostic gates retain their existing policies.

Session end and recording readiness are separate lifecycle facts. `session_finished` ends live
semantic processing and changes the recording display to `finalizing`. Only after the joined
session directory is atomically published does `recording_ready` expose its directory and manifest
digest. That immutable directory is importable while the parent watcher continues waiting for a
future Gamescope session. Missing components or publication failure changes the display to
`degraded` instead. The live loop polls asynchronous recorder health independently of frame
admission and emits a final health sample after encoder shutdown, so an encoder-side failure is not
hidden when capture has stopped.
Recording health and finalizing events carry the same session ID and capture generation as every
other stored run event, so the joined v5 event stream remains directly verifiable and importable.

The first encoder start, write, or close failure is terminal for that session's canonical pixel
stream. The recorder marks subsequent retained frames lost while draining their metadata without
starting another FFmpeg child. Finalization therefore pays at most one bounded encoder timeout
rather than one timeout for every frame admitted before the failure became visible. Every encoder
start/finalize error path kills and reaps an owned child and removes its unpublished segment file.

## Consequences

Normal encoding jitter can consume the explicitly allowed memory instead of losing evidence at an
arbitrary two-frame boundary. The configured limit is still a hard admission boundary for
recording-owned memory, so memory use cannot grow without operator control. A limit crossing is
visible while the session is active and cannot be mistaken for a complete corpus candidate.

Import takes longer because it owns lossless decode verification, but that work is outside gameplay
and is exactly where active-suite admission already depends on it. A published partial session may
still be inspected, but only a complete, decode-verified session may be applied.
Numeric dataset authoring performs one screen pass and one selected-frame pass per session, not one
full decode per labeled RESULT episode.

This supersedes ADR 0101 only for canonical queue capacity, segment closure at intentional gaps,
realtime decode verification, and ordinary-run QOI retention. ADR 0101 remains authoritative for
opt-in recording, retention windows, shared timeline replay, OCR scheduling, and label v5 truth.
