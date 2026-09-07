# ADR 0141: Bound Gamescope capture size and frame copies, then quiesce disconnect

- Status: Accepted
- Date: 2026-09-08
- Supersedes: ADR 0027, ADR 0070, ADR 0072 and ADR 0073 only for an output-sized Gamescope
  capture contract, copying one frame on every PipeWire process callback, and immediately
  disconnecting an active stream
- Complements: ADR 0056's 10 Hz recognition cadence and ADR 0029's fail-closed profile admission

## Context

Gamescope `3.16.19-128-g7282613+` advertises an unspecified `0/1` PipeWire rate. Its source ignores
the negotiated video framerate and drives capture from every physical output vblank. A
5120x1440@120 Hz development-host run consequently delivered about 120 callbacks and full-frame
copies per second even though scorepeek preferred 10/1 and recognition consumed at 10 Hz.

The same host reproduced the reported lifecycle failure without INFINITAS: after a legal consumer
disconnect, Gamescope sometimes aborted in `destroy_buffer`. Gamescope exposes the private SPA
format property `SPA_FORMAT_VIDEO_requested_size` (`0x70000`). It clamps the capture to that
rectangle while preserving source aspect ratio, but it does not use that property or the negotiated
framerate to pace capture. Applying it also produces a startup renegotiation: Gamescope can announce
the full output first, mark the stale transition buffer corrupted, and then announce the bounded
contract.

Capturing a 4K or 5120x1440 BGRx frame has no recognition value because the canonical recognition
input is fixed at RGB8 1920x1080. Output geometry and the observed PipeWire capture geometry must
therefore be distinct profile facts.

## Decision

Every Gamescope receiver offer includes `requested_size=1920x1080` in addition to BGRx and the
preferred 10/1 rate. Gamescope may preserve aspect ratio, so a 16:9 source should negotiate
1920x1080 while a 5120x1440 source should negotiate 1920x540. There is no full-size fallback.

Before the first valid copied frame, the receiver permits Gamescope to replace its announced video
contract and returns a producer-marked corrupted transition buffer without admitting it. The first
valid frame fixes the contract. Any later contract change, corrupted buffer, memory-type change,
stride change or malformed extent remains a steady-reception failure. Negotiation diagnostics are
published only after the first valid frame and describe that frame's final contract.

Continue to dequeue the bounded available PipeWire set and return every buffer before the process
callback ends. Validate the newest buffer on every callback, then copy it into application-owned
memory only when the next fixed 10 Hz monotonic deadline is due. Advance late deadlines by whole
intervals so callback jitter does not permanently shift the cadence. Receiver sequence,
`received_frames`, latest-frame replacement and maximum-gap diagnostics describe copied application
frames.

Before disconnect, deactivate the stream on its owning PipeWire thread, iterate the same main loop
until the stream leaves streaming state within 100 ms, and service one further bounded 25 ms grace
period before explicit disconnect. Failure to deactivate, reach quiescence, service the loop or
disconnect remains a receiver shutdown failure. Stream, listener and provider destruction order is
unchanged.

Output dimensions in existing calibration provenance no longer have to equal the observed video
contract. Existing profiles calibrated against full-size 4K or ultrawide frames do not match the new
observed dimensions and intentionally fail admission. The operator must rerun
`scorepeek setup gamescope` to measure and register the bounded capture domain; scorepeek does not
rewrite or silently reinterpret an old binding.

## Consequences

- 16:9 capture bandwidth and each admitted application copy are bounded to BGRx 1920x1080. Other
  source aspect ratios can be smaller on one axis and still require their own measured normalizer.
- A 120 Hz producer can still render and supply PipeWire buffers at its own cadence, but scorepeek
  performs at most about ten full-frame application copies per second and promptly returns the rest.
- Initial Gamescope renegotiation is explicit and bounded; the established capture profile remains
  fail closed.
- Deactivation and bounded grace reduce exposure to Gamescope's in-flight buffer teardown race but
  cannot prove or repair producer correctness. Repeated lifecycle gates and Gamescope logs remain
  required evidence.
- The public event API and diagnostic schemas remain unchanged. The meaning of the existing
  negotiation dimensions stays the actual frame-producing PipeWire contract.
