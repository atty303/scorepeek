# Overlay visual debugging

DOM and bounding-box assertions do not detect every native rendering problem. The native visual
debugger renders the production Dioxus component through the native DOM, Blitz paint and Vello image
path without connecting to Wayland. A scenario retains UI state across clicks, scrolls and drags and
writes a PNG plus selector layout JSON after every operation. `manifest.json` correlates the
artifacts and records completeness and a typed failure.

Run the checked-in representative scenario into a new output directory:

```text
mise run overlay:visual:native -- crates/scorepeek-overlay/tests/fixtures/visual-debug.json /tmp/scorepeek-native-visual
```

The output directory must not already exist. A scenario retains one image renderer for its complete
action sequence. Scenario actions are `set_editing`, `set_screen`, `click`, `scroll`, `drag`, and
`capture`. `set_screen` accepts a screen kind or `null` and exercises normal runtime canvas
visibility. Clicks use CSS selectors. Left drag moves the widget under its starting point; right drag
moves the selected canvas. Coordinates and layout rectangles are logical CSS pixels. PNG dimensions
are the logical output size multiplied by `scale` and rounded up; the manifest records both sizes.
This path is intended for visual diagnosis and does not save overlay configuration.

For the OBS route, give the server a new dedicated configuration path and optionally a loopback
listen address. The configuration file must not already exist, and non-loopback addresses are
rejected:

```text
mise run overlay:visual:obs -- /tmp/scorepeek-obs-visual/overlay.toml 127.0.0.1:17384
```

Open `http://127.0.0.1:17384/overlay` in Codex Browser, set its viewport to the configured logical
output size, and use the Browser DOM, interaction and screenshot surfaces. Press Enter in the server
terminal to stop it. The server uses the production `/overlay`, iframe canvas, WebSocket and editor
code, but an absent event socket and no score database, so it starts with the editor's fixed sample
data and never initializes capture, recognition, Wayland or OBS.

Native and browser images are evidence for human or Codex comparison; pixel equality is not an
acceptance condition. Actual Wayland composition/input delivery and rendering inside OBS remain live
verification boundaries.

A scenario may set `skin` to `cyan-system`, `result-aurora`, or `dj-blackbox`.
`{"action":"motion","seconds":2.25}` samples the production native motion driver at an explicit
nonnegative timestamp and records another PNG/layout pair. Use multiple timestamps to inspect
changing paint as well as stable text. The action is a development capture clock, not a runtime setting.

## Stream composition scenario

`tests/fixtures/visual-composition.json` supplies synthetic `canvases` (the shared presentation
schema) rather than the normal first-run canvases. This optional scenario field does not change the
runtime defaults. It exercises background motion, S/M/L changes and a switch back to static:

```text
mise run overlay:visual:native -- crates/scorepeek-overlay/tests/fixtures/visual-composition.json /tmp/scorepeek-composition-visual
```

Inspect PNG alpha in game/camera/comment interiors, the chamfered opaque corners, background in
narrow gaps, labels, fixed content rectangles across frame changes, and the manifest/layout pairs.
Repeat with each `skin` override. OBS uses the same aperture asset route and background composition.
While editing, each visible EMPTY aperture also shows compact viewport-relative
`x,y · width×height` geometry. Confirm that it follows move and resize. In OBS these logical values
are relative to `/overlay`; Browser Source scene transforms remain OBS-owned and are not displayed.
In the OBS editor, add EMPTY, enter or clear TITLE, change INTERIOR OPACITY, select FRAME WIDTH and
ASPECT RATIO, then save/reopen and discard another change. Place an OBS source under the browser
source to confirm that the aperture and fill expose/dim it.

The composition fixture also exercises the native title editor with a synthetic IME preedit and
commit batch via `title_text` (`composing: true` previews conversion). It uses the production title
buffer and UI; it does not establish compositor keyboard focus or real IME candidate placement.

## Shared editor UI

Native and OBS render the shared Dioxus `EditorPanel`, `EditorSurface`, `EditorCanvas`,
`ResizeHandles` and `PlacementPreview` components in `scorepeek-overlay-ui`. The shared
`editor_model::Model` owns selection, settings, placement and gesture transitions, including
four-corner resize, aspect ratios and canvas bounds. `EditorButton` owns text alignment,
sizing, selected/disabled state and tone; parent CSS owns placement and spacing.

The native adapter retains pressed buttons across motion/release, translates Wayland
pointer input to Blitz `UiEvent` and lets Dioxus
hit testing, bubbling and component callbacks produce the same typed actions as the web
renderer. The native visual scenario also sends these events; it does not interpret
selector names as setting commands. Native IME buffers and browser input elements adapt
platform text entry. Output/surface ownership, keyboard focus, pointer capture and the
existing save/lease transports remain host responsibilities.

OBS `/overlay` boots the canvas WASM bundle and places display-only canvas iframes inside
the shared editor canvas. Native supplies rendered canvas content in the same component
slot. This content layer is noninteractive and isolated below the shared hit regions.
Selected widget handles take precedence when they coincide with canvas corners; selecting
the canvas in the panel clears widget selection and exposes canvas handles. Widget artwork
and content use the same `overlay_canvas` renderer in both routes.
Browser capture and actual Wayland composition/input remain separate verification gates.

`tests/fixtures/visual-empty-editor.json` starts with no visible canvas and toggles the
last canvas on/off. Inspect the panel, scroll body and footer in every PNG/layout pair;
the shared root stylesheet must remain present independently of canvas visibility.

## Browser build compatibility

The overlay backend and browser WASM compile the same SHA-256 build identity from
`scripts/overlay-build.rs`. Its inputs cover overlay sources, shared assets/styles,
workspace dependencies and build configuration; this is a build identity, not the
configuration revision. Rebuild the web bundle before building the embedded backend.

Each stage connection and editing request must match the backend build identity.
Until the first matching stage arrives, editing is disabled. A mismatch discards the
local draft, selection, gestures, title editing and undo history and displays a
reload button. It does not save, restore or automatically reload the draft. Reload
starts from persisted settings. Display-only canvas URLs are not editor sessions.

The server owns each WebSocket's editor identity and releases that connection's
lease on disconnect, including mismatch rejection. Reconnecting cannot release
another connection's lease. A waiting editor retries acquisition periodically.
The private child diagnostics report `overlay_editor_version` (success or
`version_mismatch`) and `overlay_editor_connection` release outcomes; the existing
controller records lease and commit outcomes.

For upgrade verification, keep a page open while replacing the isolated test
server with another build. Confirm the mismatch notice, absence of editor handles
and absence of configuration writes, then reload against matching assets and edit
again. Versions predating this guard cannot render the new reload notice, but their
unversioned editor requests are rejected by the new backend.
