# ADR 0104: Pipeline replay preparation and field projection

## Status

Accepted

## Context

ADR 0103 shared OCR workers across replay sessions, but one session still invoked screen
classification, crop preparation, field admission, and ordered timeline processing synchronously
from the FFmpeg stdout callback. Increasing text workers beyond four did not shorten that producer
path. The 1,061-frame corpus used about 200 seconds of aggregate CPU work on a Ryzen 9 9950X3D
host exposing 32 logical CPUs, with unrestricted affinity and `cpu.max=max`; the earlier
interpretation that the host exposed roughly two effective CPUs was false.

After separating decode measurements from callback backpressure, pure FFmpeg decode took about
14.7 seconds while the production replay took about 59 seconds. Per-stage measurements then showed
that full-catalog projection and join were also serialized by one outer field worker even though
text and numeric inference had already been submitted independently.

## Decision

Canonical replay submits decoded frames to one global pure-preparation pool. The pool classifies
the canonical RGB8 frame and prepares applicable crops without mutating diagnostic, screen episode,
attempt, or event state. A session-local reorder queue admits prepared values only in source
sequence. Timeline actions, close-time drain, RESULT finalization, and domain-event publication
remain exclusively ordered and session-local. The pool uses
`clamp(available_parallelism / 4, 1, 8)` workers and consumes the existing global replay memory
account; pressure blocks the decoder callback and never drops an offline frame.

The registered field observer may fork only explicitly frame-local outer workers. Live uses at most
two outer workers, matching its two-frame outstanding limit. Offline replay uses at most four per
session. Text ONNX sessions remain in the global pool, numeric inference remains on its registered
single-thread worker, and every ONNX session retains intra- and inter-op thread count one. Worker
completion may be out of order, but pending handles retain their original binding and the session
committer consumes them only in source sequence.

Full-catalog edit distance uses a one-row dynamic program with stack storage for ordinary bounded
titles and heap fallback only for longer inputs. Scoring a frame may use at most four scoped song
chunks, with a process-global cap on additional catalog workers. A four-entry exact-input cache may
reuse immutable catalog projection without reusing timing, numeric inference, or frame authority.

Replay summary v3 additionally reports raw summed durations for decoder-consumer wait,
preparation queue/wall time, classification, crop preparation, field queue wait, numeric
inference, join, and catalog projection, plus the maximum per-frame text-worker inference time.
FFmpeg child wall time remains child lifetime and is not presented as pure decode CPU time.

## Consequences

The current 1,061-frame suite emits the same five accepted events with one and twelve text workers.
Three runs have medians of 86.65 seconds with one text worker and 31.21 seconds with twelve; the
prior default median was 57.92 seconds. This establishes useful parallel speedup but does not
satisfy the 20-second prospective gate. Target authority, install, push, and release remain
separate approval boundaries.

The added parallel work is bounded by CPU-derived worker counts, session outstanding limits, and
the existing 2 GiB default memory account. This supersedes ADR 0103 only for synchronous replay
preparation and the single outer field worker; its shared text pool, multi-session decoder policy,
ordered commit contract, and no-drop offline behavior remain in force.
