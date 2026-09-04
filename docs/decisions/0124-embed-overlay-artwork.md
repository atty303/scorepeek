# ADR 0124: Embed decorative overlay artwork

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0123's raster-free skin treatment, following the request to approach the original concepts with more imagery.

## Decision

Bundle original generated PNG frames for CYAN SYSTEM, RESULT AURORA and DJ BLACKBOX,
and a separate aurora header. Keep all labels, numbers and state as shared semantic text.
No game captures, player data or game assets are included. Record provenance, dimensions
and content hashes beside the unmodified artwork.

CSS slices the frame into nine backgrounds, retaining corner geometry while the central
area and straight edges adapt to compact/sidebar layouts and variable text height. The
same decorative DOM and CSS serve native and browser rendering. There is no per-skin
screen implementation, user asset loader, runtime download or continuous animation.

The fixed artwork registry supplies bytes to the native renderer and the OBS HTTP asset
handler. Native enables only PNG decoding on the existing image crate and ingests synchronous
image completions before the first paint; no extra redraw loop is required. Appearance selection, public events, confirmation and saved-history authority
remain as defined in ADR 0123. Existing private appearance and renderer observations
remain separate from displayed snapshots; no image content enters diagnostic events.

## Verification boundary

Inspect all three skins in both layouts through the native Vello preview and embedded
browser UI. Exercise embedded PNG delivery and unknown-asset 404 through the real OBS
child, including the moved distribution binary. Retain the existing layout, state and
finite-animation checks. Real fullscreen placement, input passthrough, scaling and GPU/OBS
performance remain the explicit target live gate from ADR 0122.
