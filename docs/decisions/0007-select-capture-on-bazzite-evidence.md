# ADR 0007: Select one post-scale capture backend from Bazzite evidence

- Status: Accepted
- Date: 2026-08-15
- Supersedes: ADR 0002

## Context

The required recognition domain is the Gamescope-scaled output. OBS game
capture normally sees a native FHD swapchain before scaling. Gamescope's direct
PipeWire capture and compositor output are related but not guaranteed to be
pixel-identical. Their load also differs on the target machine.

## Decision

Use Wayland ScreenCast Portal as the correctness reference. Compare Gamescope
direct PipeWire and an OBS path only when the latter is proven to share a 4K
post-scale source already used for streaming. OBS WebSocket screenshots remain
diagnostic only.

All candidates produce a versioned 3840x2160-post-scale-to-1920x1080 canonical
profile. Adopt a non-Portal default only after it passes geometry, semantic
recognition, lifecycle, and target performance gates and materially reduces
resource cost. Never switch profiles silently during a session.

## Consequences

- The default backend is decided on the actual Bazzite machine, not this
  development host.
- Native FHD, unknown color/format, and unverified OBS sources fail closed.
- If no alternative proves both correctness and lower load, Portal remains the
  supported default.
