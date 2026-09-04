# ADR 0123: Add CSS overlay skins and independent layouts

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0122's fixed three-card appearance and system-font-only rendering.

## Decision

Keep one shared Dioxus presentation for native and OBS, with three embedded CSS skins:
`cyan-system` (default), `result-aurora`, and `dj-blackbox`. Choose once at startup with
`--overlay-skin`. Each child receives the same skin and its selected layout:
`--overlay-wayland-layout compact|sidebar` defaults to compact;
`--overlay-obs-layout compact|sidebar` defaults to sidebar. No external CSS loader or live
settings service is introduced.

Live, latest confirmation, and selected-chart best/latest five saved plays remain present in
both layouts. Semantic field roles separate the main score, rank, miss and clear from supplemental
judgments/timing/options; styling never interprets translated labels. Result headers use result
identity rather than an unrelated retained selection. Saved history remains associated with the
selected chart. Unknown values use an em dash, never zero; SELECT best remains receipt indicators.
Best fields have no shared achievement date. History dates remain UTC notification timestamps.

Common layout CSS owns structure and spacing. Skin CSS supplies colors, gradients, clipped frames,
shadows and typography through a root attribute and CSS variables. No raster imagery, game assets,
continuous effects, blur, or renderer-specific UI tree is required. Confirmation has a finite
450 ms opacity transition and respects reduced motion in supporting renderers.

Embed the unmodified Oxanium variable TTF with its OFL license and source revision/hash under
`crates/scorepeek-overlay-ui/assets/fonts`. Native registers its bytes alongside system fonts;
OBS serves the same bytes at `/fonts/oxanium.ttf` and loads them via `@font-face`.
The embedded license is available at `/fonts/OFL.txt`. Japanese falls
back to system fonts. Font installation and runtime network acquisition are unnecessary.

Appearance is independent of public v1 events and the score database. The parent passes typed
appearance with the existing private child configuration. The OBS index injects enum-only initial
metadata and disables caching; the browser validates it before connecting. Existing private child
observations record `appearance_selected` (skin/layout only), with existing opt-in recording,
retention and failure non-interference. Diagnostics are not added to display snapshots.

## Verification

Use shared synthetic state for native DOM layout/settled-animation checks and native Vello preview
rendering. `mise run overlay:skin:render INPUT.json OUTPUT.pam` creates an RGBA PAM without Wayland,
recognition or a score DB; an example input is `crates/scorepeek-overlay/tests/fixtures/skin-preview.json`.
The render command is an explicit GPU task, not part of normal automated tests.

Check CLI rejection/defaults, initial appearance and font delivery through a real child, moved-binary
assets, and existing provisional/reconnect/history behavior. Inspect all six skin/layout combinations
with Japanese text and long titles. Native/browser agreement means equivalent structure and readable
information, not pixel identity. Target fullscreen placement, input passthrough and CPU/GPU/OBS lag
remain the explicit live gate in ADR 0122, using a dedicated score DB.
