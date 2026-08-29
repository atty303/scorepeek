# ADR 0073: Service PipeWire on a dedicated loop

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0070 and ADR 0072 only for assuming that process-event buffer return on the
  application-driven loop is timely
- Complements: ADR 0027's provider/receiver boundary and ADR 0056's 10 Hz application sampling

## Context

The target run after ADR 0072 negotiated Gamescope's BGRx 3840x2160 `0/1` format and received
33,826 frames, so format compatibility and buffer mapping succeeded. Gamescope nevertheless kept
reporting `warning: out of buffers` and `push_pipewire_buffer: Already had a buffer?!`.

Gamescope advertises four buffers with an allowed range of one through eight. Its compositor asks
for a PipeWire buffer, but its compositor-to-PipeWire handoff has only one pending slot. The latter
warning means a second filled buffer reached that slot before the Gamescope PipeWire thread consumed
the first. The out-of-buffers warning means no reusable PipeWire buffer was available when the
compositor requested one.

Scorepeek drove the provider registry and receiver stream from one manually iterated PipeWire main
loop on the foreground capture path. That same path performs normalization and coordinates
recognition. During those operations no PipeWire callbacks ran; when iteration resumed, the
consumer could drain and return several accumulated buffers together. Returning buffers inside the
process callback was locally correct but did not make callback scheduling independent or timely.
Changing the offered framerate or buffer count cannot repair that scheduling dependency.

PipeWire's threaded-loop design exists so a synchronous application can use the asynchronous API
without stalling the library. The Rust binding's threaded-loop constructor is unsafe, while this
repository prohibits unsafe code.

## Decision

Run the receiver's complete PipeWire ownership graph on one dedicated safe Rust thread: main loop,
context, core, stream, listener, negotiation, mapped-buffer processing, explicit disconnect, and
drop. No PipeWire object crosses that thread boundary.

The process callback keeps the PipeWire reference behavior: dequeue the bounded available set,
return every superseded buffer, copy only the newest valid mapped frame into the one bounded
application-owned latest-frame slot, and return the final PipeWire buffer before the callback ends.
The foreground recognition path may take or replace that owned frame but cannot stall the receiver
loop.

After a raw BGRx format is accepted, the consumer calls `pw_stream_update_params` with its buffer
contract: the producer's four-buffer preference and one-through-eight range, one block, exact
`width * height * 4` size, exact `width * 4` stride, and MemFd data. This follows PipeWire's format
negotiation contract and intersects Gamescope's allocation offer without using buffer count as a
pacing mechanism. The format is committed and reported as negotiated only after that buffer update
succeeds.

The existing provider lease remains a separate default-remote connection. Taking a latest frame
also advances its registry loop without waiting, so continuous receiver delivery cannot starve
selected-node lifetime observation. Shutdown sends one bounded command to the receiver thread,
disconnects and drops the stream there, and then releases the provider lease. A bounded shutdown
timeout is an error and does not claim that the thread terminated. A process-wide supervisor stays
owned by that thread until its actual exit, so a residual receiver prevents a successor receiver
from starting and cannot overlap another consumer.

Shutdown first advances the provider registry loop once without waiting. A selected-node removal
pending since the last foreground poll is therefore recorded by the provider before receiver
disconnect and provider release.

## Consequences

- PipeWire buffer service no longer depends on OCR, normalization, diagnostic recording, or the
  10 Hz recognition cadence.
- The negotiated producer rate, including `0/1`, remains an observation; recognition still samples
  only the latest owned frame at 10 Hz.
- Buffer negotiation is explicit, but buffer count is not used to conceal producer or consumer
  scheduling defects.
- Negotiation and first-frame deadlines latch the first terminal cause in the shared receiver state
  before shutdown. A terminal wins over concurrent startup readiness, first-frame success is
  recorded before the frame becomes available to the caller, and shutdown records a terminal
  latched immediately before worker exit.
- The existing first-frame, steady-reception, maximum-gap, stream-loss, source-loss, and shutdown
  facts remain the bounded diagnostic surface; no raw PipeWire or Gamescope log is copied into the
  public result surface.
- Repository tests can verify ownership, bounded state, negotiation pods, and teardown paths. A
  target run must still establish that both Gamescope warnings and the scorepeek node's increasing
  `pw-top` error count stop under real recognition load.
