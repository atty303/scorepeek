# ADR 0138: Compose stream backgrounds and empty widgets

- Status: Accepted
- Date: 2026-09-07
- Supersedes: ADR 0125's outer widget geometry and always-transparent canvas background.

## Decision

All three skins support the same composition in Wayland and OBS. A canvas owns a bundled,
skin-specific background with `none`, `static` and `animated` choices. Enabling a background normally
selects animated; an existing or newly created canvas starts at `none`. Background images contain
material and light only; game, camera, comments, titles and layout are not baked into them. No image
import or stream layout preset is added.

An `empty` widget supplies an image-backed frame and optional arbitrary title. An empty title removes
the tab. The interior is transparent by default; `fill_opacity_percent` from 0 to 100 adds a black
fill over content below it. Its chamfered aperture also subtracts from the canvas background.
Overlapping apertures subtract their union. In OBS, the single `/overlay` Browser Source sits above
the game, hand camera (SP or DP), face camera and comments sources. Those sources remain OBS-owned.
The frame and its title do not capture or embed media. Wayland can use the same composition or simply
leave backgrounds disabled.

Every widget has `frame_width = "s" | "m" | "l"`. Its stored x/y/width/height describe the inner
rectangle before corner chamfers. Frame extent grows outward, with nominal insets 4/8/16 logical
pixels. M retains the current existing-widget artwork and content layout. Changing the frame never
moves or scales the content. Decorations may extend beyond widgets but the canvas clips them;
operators adjust canvas and widget placement visually.

Empty widgets resize freely or lock to 16:9 (`wide`), 4:3 (`standard`), or the inner dimensions captured
when CURRENT is selected (`current = [width, height]`). Resize operations retain the opposite corner
and fit within the canvas. Dimensions snap to the shared 4px grid, so ratios have at most one grid
cell of rounding error. Titles use system Japanese fallback rather than bundling another Japanese
font. Existing score/information widgets retain their semantic content and typography.

Schema version 5 migrates versions 2–4 through their existing transitions, then maps widget outer
geometry to inner geometry: x/y +8 and width/height −16. M and background none are the migration
defaults. The existing atomic writer publishes the migrated document only after validation. Backend
draft leases, revisions, discard and save continue through the parent controller.

## Verification boundary

The shared DOM/CSS and motion specification are used by native and WASM; Rust and JavaScript drive
the same visual concepts. The native visual debugger accepts explicit synthetic canvases for
composition, opacity and clipping checks. PNG alpha must confirm apertures; positive rectangles do
not establish transparent paint. Browser/OBS and real Wayland compositor checks remain separate
from headless native rendering. Configuration tests cover geometry migration, idempotent reload,
and presentation/storage round trips.
