# ADR 0140: Cap Wayland overlay rasterization

- Status: Accepted
- Date: 2026-09-07
- Supersedes: ADR 0131's unconditional native visible-surface animation cadence.

## Decision

The Wayland overlay has one backend-wide `wayland_refresh_hz` setting. `"auto"` retains the
compositor-paced behavior; an integer from 1 through 1000 is a maximum rasterization rate for every
visible Wayland canvas. The editor exposes AUTO and an exact integer draft. Incomplete, non-integer
and out-of-range drafts remain visible with an inline error and disable SAVE; they are not clamped.
Changing this setting participates in the existing backend-wide draft, one-step undo and atomic save.
The editor itself always renders compositor-paced so direct manipulation stays responsive.

Accepted public events and Dioxus state updates remain event-driven. When a cap is active, multiple
state or animation changes coalesce into the next permitted paint. Motion samples monotonic elapsed
time at that paint, so a lower cap skips visual samples instead of slowing animation. Initial surface
configuration, size or scale reconfiguration, the one transparent commit when a canvas becomes
hidden, and editor paints bypass the cap. Hidden surfaces remain idle after their transparent commit.

OBS has no scorepeek refresh-rate setting. Its shared DOM continues to schedule motion with
`requestAnimationFrame`, allowing the OBS Browser Source custom frame rate to own capture cadence.
Scorepeek does not know an OBS source's scene Position, Scale, Crop or Bounding Box transform.

Schema version 5 is retained. New and newly saved documents always contain
`wayland_refresh_hz = "auto"` or an integer. Loading an existing schema-v5 document without the field
atomically adds `"auto"`. Consequently an older schema-v5 reader that rejects unknown fields cannot
read a document saved by this implementation; this compatibility break is accepted without an alias
or parallel schema.

While either backend editor previews the current game screen, every visible empty widget shows a
compact `x,y · width×height` overlay for its inner aperture. Coordinates are logical pixels from the
top-left of the `/overlay` viewport or Wayland output preview and update during move and resize.
There is no label in normal display. These values help reproduce geometry in OBS, but do not claim to
be OBS scene-transform coordinates.

## Observation and verification boundary

The existing bounded native summary records the configured mode/value, elapsed time, total and
steady paint counts, effective rates, and cap-bypass counts grouped by reason. Controller diagnostics
continue to record atomic save outcomes. They are observational and cannot change paint admission or
event delivery.

Deterministic development tests cover AUTO, exact cap boundaries, coalescing, lifecycle bypasses,
strict configuration values, v5 field insertion, draft/undo/save ownership and viewport-relative
empty geometry. Native and browser visual scenarios verify compact readable overlays and shared DOM.
Actual rasterization savings, Wayland compositor behavior, OBS rendering and OBS lag remain explicit
target-live performance gates.
