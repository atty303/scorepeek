# ADR 0127: Switch overlay canvases by semantic screen

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0125's one-canvas initial layout and always-visible surface behavior

## Decision

Overlay TOML schema version 2 gives every canvas an optional `show_on` list containing
`music-select`, `mode-select`, `decide-transition`, `play`, or `result`. Omission means always
visible and an empty list is invalid. The public event v1 stream adds a nullable `screen_state`
snapshot slot and `screen_state_changed` live event projected from semantic screen episodes.
Started/resumed episodes are active, suspension starts the global `unknown_grace_ms` interval,
finalization clears the screen, and closing does not change visibility. A new known screen replaces
the previous screen immediately. Socket loss uses the same grace interval; explicit session finish
clears it immediately. Recognition remains the sole screen authority.

The initial schema-v2 document contains four canvases for each enabled backend: an always-visible
status canvas, a MUSIC SELECT dashboard, a compact DECIDE/PLAY selection canvas, and a RESULT
dashboard. MODE SELECT consequently shows only status. Existing `/canvas/<id>` OBS URLs obey the
same visibility rule. `/overlay` is the full-screen OBS Browser Source URL: it places all enabled
OBS canvases in a 1920x1080-oriented logical pixel space, clips at the browser viewport, and orders
them by canvas `z`. It reuses the individual canvas documents inside the single Browser Source, so
right-clicking a canvas opens its existing editor; right-clicking empty stage space opens the canvas
list and can preview a hidden canvas.

Wayland continues to own one layer surface per enabled canvas, but all surfaces share one event
feed and visibility clock. A hidden surface commits transparent content once, gets an empty input
region, and then stops painting. An editor preview overrides `show_on`, transfers the edit lease,
and restores normal visibility on DONE. Surface ordering follows `z` as a best-effort creation order;
the layer-shell protocol does not guarantee ordering among peer layer surfaces.

Wayland canvases also have `opacity_percent` from 1 through 100. It affects canvas content while
editor controls and the pointer remain opaque; OBS requires 100. The editor exposes screen filters,
opacity, `z`, and unknown grace without keyboard input. Wayland requests compositor cursor shapes
for normal, move, grab, and resize states and uses an embedded 24-logical-pixel arrow buffer when
the cursor-shape protocol is unavailable. Hidden surfaces receive no pointer input. While editing,
a compact Wayland surface temporarily expands within its output and an `/overlay` iframe temporarily
occupies the full OBS stage so the same in-canvas controls remain usable; DONE restores saved geometry.

Schema version 1 is rejected without migration. A missing document creates the version-2 defaults.
An absent or disconnected Wayland output still resolves to an available named output and persists
that replacement. The backends remain independently enabled by their existing run flags.

## Consequences

Selection, play, and result can use different positions and widget compositions without duplicating
display state or recognition logic. The public socket remains event schema v1 and the new event is
additive, but consumers that want screen-aware presentation must fold its nullable retained slot.
Existing schema-v1 overlay TOML must be replaced or manually rewritten as schema v2.

## Verification

Development verification covers strict schema-v2 defaults and rejection, screen filtering,
semantic episode projection, suspension/disconnect grace, new-screen replacement, shared browser
stage assets, input-region and cursor protocol compilation, and all existing overlay tests. Target
verification remains required for peer layer ordering, cursor fallback appearance, fractional scale,
opacity, OBS Interaction, and idle GPU behavior.
