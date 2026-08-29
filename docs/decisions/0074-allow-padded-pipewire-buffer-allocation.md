# ADR 0074: Allow padded PipeWire buffer allocation

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0073 only for requiring exact buffer size and stride

## Context

The first target run of the ADR 0073 receiver connected and completed 588 recognition ticks over
71 seconds. When Gamescope exposed a later source, PipeWire rejected link allocation with `-22`.
Its retained daemon log shows the scorepeek input buffer parameter derived from a 3828x2058 BGRx
format: size 31,512,096 and stride 15,312. Gamescope offered a 3840x2160 backing allocation: size
33,177,600 and stride 15,360. The exact values had no intersection.

Buffer memory dimensions are allocation properties, not a second assertion of visible video
geometry. A producer may provide a larger backing block and stride for alignment or padding. The
received chunk still carries the actual offset, size, and stride that the consumer must validate.
PipeWire's GStreamer integration consequently offers minimum buffer size and an open stride range
instead of requiring both to equal the visible format footprint.

## Decision

Keep the dedicated receiver loop, buffer-count range, one block, and MemFd requirement from ADR
0073. Offer `width * height * 4` as the minimum buffer size with scorepeek's existing 128 MiB frame
bound as the maximum. Offer `width * 4` as the minimum stride and `i32::MAX` as the protocol-level
maximum.

Do not infer the copied image layout from those offered defaults. Continue to validate every
received chunk's positive stride, offset, mapped length, minimum row width, and bounded
`stride * visible_height` before copying. Profile admission continues to bind visible negotiated
width and height while treating allocation padding as incidental.

## Consequences

- A producer may allocate a padded BGRx backing store without making the PipeWire link
  unsatisfiable.
- A buffer smaller than the visible BGRx footprint remains outside the offer and is also rejected
  at receipt.
- The application memory bound remains 128 MiB even though the SPA stride range uses the protocol's
  full positive integer range; receipt validation prevents an oversized copy.
- Target validation must cover the observed 3828x2058-visible/3840x2160-backing transition as well
  as steady 3840x2160 capture and Gamescope buffer warnings.
