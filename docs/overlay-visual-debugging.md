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

For a bounded live Wayland check, run the production editor against an isolated empty config. The
second argument is the lifetime in seconds. This route discovers compositor outputs, opens the
empty-workspace editor automatically, and persists only to the supplied path:

```text
mise run overlay:visual:wayland -- /tmp/scorepeek-wayland-visual.toml 60
```

To reproduce startup and selection cost with an existing current-schema document without modifying
it, pass it as the third argument. The first path is still a create-only shadow copy and is the only
configuration the run may save:

```text
mise run overlay:visual:wayland -- /tmp/scorepeek-wayland-latency.toml 60 /path/to/source-overlay.toml
```

When compositor nesting is part of the test, launch this task inside the nested compositor's own
`WAYLAND_DISPLAY`; record that display and the compositor process independently from the parent
desktop session.

The native child emits timestamped `native_startup_timing` records for shell connection, renderer
creation, application initialization and first paint. `elapsed_us` is measured from that surface
worker's start; renderer creation and paint records report the duration of their own operation.
`native_skin_runtime_timing` separately reports in-process cache hits,
waits and cold Cranelift compilation, including engine and module time, only from the native child
diagnostic path. Editor button actions emit a
`native_editor_interaction` `state_applied`
record followed by the first corresponding `painted` record. Their shared `run_id` and
`interaction_id` correlate the operation, while `action_us`, `control_us`, `skin_us`, `dioxus_us`,
`paint_us` and total `duration_us` localize latency. Control and skin components
also emit individual timing records with success or a stable error type. These records contain only
stable action/request names and operational canvas/output identifiers; they do not record titles,
property values or other entered content. Up to 64 actions awaiting paint retain distinct
correlations; overflow emits a typed `interaction_queue_full` dropped record instead of silently
replacing an earlier action. The child-to-parent queue remains bounded and uses the existing local
diagnostic recording path.

Multi-output editor shutdown is recorded as `native_editor_workspace_transition`,
`native_editor_stage_transition`, and `native_editor_stage_shutdown`. The workspace record
identifies the close reason and initiating output. Each stage then records its editor-to-display
transition. When the editor closes, topology changes or the parent lease closes, every affected stage
records `stop_requested`; terminal `stopped` includes the output, status and duration, and
`native_renderer_shutdown` separates app-loop completion, renderer suspension and
surface teardown. `native_surface_unmap` then records publication of the buffer detach after the
renderer has released its Wayland surface resources. A missing phase or unmap failure therefore
distinguishes a worker wake failure, renderer teardown stall and Wayland publication failure
without adding debug text to the overlay UI. An unmap publication failure also makes the worker and
native summary fail with `wayland_surface_unmap_failed`; it cannot be reported as a successful stop.
If the app loop and unmap both fail, unmap remains the primary `failure_type` while the earlier app
loop error is retained as `secondary_failure_type` and `secondary_failure` in `native_summary`.

Each native present publishes a Wayland frame callback, including editor and visibility-clear
paints. A callback is not a request for continuous animation: it is the compositor acknowledgement
that permits a later state change to reuse frame-paced surface resources. Repeated configure events
with unchanged logical size, physical size and scale update no state and do not trigger another
paint.

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

For incremental-update verification, record the selected canvas iframe URL, move and resize a
widget, change canvas and widget properties, and inspect the iframe DOM after each action. The iframe
must keep the same URL and Wasm instance while its WebSocket applies the new presentation. A skin
change, sample-mode change, editor close/reopen, or canvas lifecycle change may create a new iframe.
Also exercise output navigation, UNKNOWN preview, add/delete/undo, save/reopen and discard/reopen;
these cover state transitions that a single drag does not.

Native and browser images are evidence for human or Codex comparison; pixel equality is not an
acceptance condition. Actual Wayland composition/input delivery and rendering inside OBS remain live
verification boundaries.

A scenario may set `skin` to any installed reverse-domain skin ID. The visual tasks build and
install the three repository packages into an isolated XDG store before running the scenario.
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

During native editing, there is one output-owned full-output stage per connected output. Canvas
assignment must update the shared draft without destroying or recreating those stages. Every local
editor transition wakes the peer stages immediately. Skin motion schedules are paused while the
editor is open; direct manipulation and state changes still request paints. In a nested multi-output
check, leave the editor idle before and after an output assignment and confirm that paint counts stop
advancing and that no configure timeout or canvas-worker failure is reported.

OBS `/overlay` boots the editor WASM bundle and places display-only skin canvas iframes inside
the shared editor canvas. Native supplies rendered canvas content in the same component
slot. This content layer is noninteractive and isolated below the shared hit regions.
Selected widget handles take precedence when they coincide with canvas corners; selecting
the canvas in the panel clears widget selection and exposes canvas handles. Widget artwork
and content come from the same package Wasm full-tree ABI in both routes.
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
