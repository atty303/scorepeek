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

The checked-in bounded nested scenario creates two headless Scroll outputs at 120 and 60 Hz, starts
the production Wayland runner with four screen-filtered fixture canvases, including an explicit
all-screen canvas, animated backgrounds, and multiple widgets split across those outputs. It drives
production `EditorInput` transport to select a named canvas, move it
between outputs, change visibility and delete it. A virtual-pointer client closes and reopens the
editor through Scroll. The virtual-pointer client then drives the public Event API through
`music_select` → `play` → `music_select`, waiting for the selection canvas's active, inactive, and
active diagnostic acknowledgements before each next step and before the editor reopens. The canvas must
unmap and remap under one native run ID, retain one skin runtime, paint within 250 ms of the second
activation, and report no display-canvas worker failure. It also
drives compositor-delivered motion, primary/secondary buttons and an
axis event with the checked-in virtual-pointer client. Every deterministic lifecycle revision must
be painted by each receiving stage; an independent compositor-input revision must be painted by the
receiving output stage within 250 ms. Both outputs must sustain at least 55 effective paints per
second, and every retained frame sample must contain every required phase:

```text
mise run overlay:visual:wayland:nested
```

This scenario is intentionally outside `mise run test`: it is used to confirm that the fake adapter
still represents the observed compositor lifecycle and frame-callback loop, while the routine lifecycle,
revision, input and retained-resource oracle remains the fake-Wayland integration test.

The native child emits timestamped `native_startup_timing` records for shell connection, renderer
creation, application initialization and first paint. `elapsed_us` is measured from that surface
worker's start; renderer creation and paint records report the duration of their own operation.
`native_skin_runtime_timing` separately reports in-process cache hits,
waits and cold Cranelift compilation, including engine and module time, only from the native child
diagnostic path. The editor causal path is
`native_editor_action_received` → `native_editor_action_reduced` →
`native_editor_projection_published` → `native_editor_projection_received` →
`native_editor_dioxus_rebuilt` → `native_editor_painted`. The editor authority records the
`session_id` and input/reducer revision, while each stage records the same `session_id`, projection
`revision`, output and native run ID at receipt and paint. `native_editor_effect` separately records
the acquire, keepalive, draft update, commit and release effect outcome and duration. These records
contain only stable input/effect names and operational canvas/output identifiers; they do not record
titles, property values or other entered content. Editor input transport is an ordered, unbounded
process-local channel, so no accepted drag movement or keyboard/IME input is intentionally dropped.
`native_summary.frame_work` aggregates call counts and nanoseconds for projection/config conversion,
Dioxus polling, package/runtime/skin-tree work, motion, resource lookup and message decoding, Blitz
layout, scene construction and renderer present/commit work. It also retains up to 256 per-frame
deltas, starting before projection acceptance and ending after presentation, records live canvas and
widget counts, and reports how many older samples were dropped. A required phase with no work is
recorded as zero; a boundary unavailable from an upstream API is listed as unmeasured.
Production scene time is separated from the renderer's combined GPU/present call. Because the
renderer dependency does not expose the Wayland surface commit as an independently timed operation,
production reports that phase under `unmeasured_calls`; the fake surface adapter measures its
separate commit operation instead of inventing a zero duration.
Package open/Arc-clone and resource lookup counts and nanoseconds are separate summary fields;
Wasm execution and JSON tree decode/validation are measured inside the runtime rather than counted
as the same elapsed interval. `native_coordinator_work` reports projection, canvas conversion and
surface-lifecycle work at the coordinator boundary.

Each active native surface presents on every compositor frame callback and publishes the next
single-flight callback with that present. The presenter has no independent refresh cap or paint
admission flag. When a display canvas becomes inactive, its current buffer is detached and committed
immediately; it does not wait for a transparent paint. Its DOM, renderer and skin runtime remain
allocated, but the renderer's GPU presentation state is suspended and both the callback-driven
presenter and the skin runtime schedule are paused. Becoming
active performs an immediate skin render, restores size, anchor, margin, exclusivity and keyboard
interactivity, and makes a bufferless commit. It waits for the layer-shell configure, resumes GPU
presentation, then presents the retained DOM to remap the surface and resumes both loops. Frame
callbacks received before that configure cannot admit the remap paint. `native_surface_transition`
records the visibility/configure/frame inputs, surface state, paint/unmap result, and renderer state
on both sides of each lifecycle boundary.
The skin schedule (`idle`, `next-frame`, or `after-ms`) drives Wasm/DOM updates and is independent of
the presenter callback rate. Repeated configure events with unchanged logical size, physical size
and scale update no state.

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
acceptance condition. Browser integration, fake Wayland, and the checked-in nested compositor
scenario are the routine completion gates. Use actual Wayland or OBS when investigating a
backend-specific failure.

A scenario may set `skin` to any installed reverse-domain skin ID. The visual tasks build and
install repository packages into an isolated XDG store before running the scenario. A skin receives
the backend and host monotonic time in every ABI input and owns its native and browser schedules.
Use captures separated by ordinary scenario interactions to inspect changing paint and stable text.

## Stream composition scenario

`tests/fixtures/visual-composition.json` supplies synthetic `canvases` (the shared presentation
schema) rather than the normal first-run canvases. This optional scenario field does not change the
runtime defaults. It exercises animated background selection and the S/M/L frame properties:

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
`ResizeHandles` and `PlacementPreview` components in `scorepeek-overlay`. The shared
`editor::model::EditorSession` owns selection, settings, placement and gesture transitions, including
four-corner resize, aspect ratios and canvas bounds. `Button` owns text alignment,
sizing, selected/disabled state and tone; parent CSS owns placement and spacing.

The native adapter retains pressed buttons across motion/release, translates Wayland
pointer input to Blitz `UiEvent` and lets Dioxus
hit testing, bubbling and component callbacks produce the same typed actions as the web
renderer. The native visual scenario also sends these events; it does not interpret
selector names as setting commands. Its scroll action resolves the selector center and sends
the production Blitz wheel event there, so a nested canvas or widget exercises ancestor scroll
targeting rather than directly mutating a scroll node. Native IME buffers and browser input
elements adapt platform text entry. Output/surface ownership, keyboard focus, pointer capture
and the existing save/lease transports remain host responsibilities.

During native editing, there is one output-owned full-output stage per connected output. Canvas
assignment must update the shared draft without destroying or recreating those stages. Every local
editor input updates the single Dioxus editor authority and publishes a complete revisioned
projection to each stage. Surface workers never acquire or release the backend editor lease and
never become editor authorities. Persistence effects and output resolution are serialized by the
coordinator; stages retain only transport replicas. Closing releases the lease before the
coordinator replaces the editor stages with display-canvas surfaces. While the editor is open, a
visible preview follows the schedule returned by its skin; damage remains
coalesced until the next callback. In a nested multi-output check, leave the editor idle before and
after an output assignment and confirm that frame callbacks continue without a configure timeout or
canvas-worker failure.

OBS `/overlay` boots the editor WASM bundle and places display-only skin canvas iframes inside
the shared editor canvas. Native supplies rendered canvas content in the same component
slot. This content layer is noninteractive and isolated below the shared hit regions.
In display mode the shared screen filter removes inactive canvas iframes from the stage DOM, which
destroys their worker-backed skin runtime; an active canvas participates in the browser/OBS
composition with no scorepeek-owned paint-rate loop. Editor preview uses the same lifecycle when
its selected screen changes.
Selected widget handles take precedence when they coincide with canvas corners; selecting
the canvas in the panel clears widget selection and exposes canvas handles. Widget artwork
and content come from the same package Wasm full-tree ABI in both routes.
The selected canvas or widget also carries one noninteractive high-contrast label with its display
name and viewport-relative `x,y · width×height` geometry. The label remains present between gestures;
dragging and resizing update the same shared DOM text. For a selected EMPTY widget it replaces the
otherwise duplicated empty-aperture geometry label.
Browser capture and native rendering remain distinct evidence surfaces.

`tests/fixtures/visual-empty-editor.json` starts with no visible canvas and toggles the
last canvas on/off. Inspect the panel, scroll body and footer in every PNG/layout pair;
the shared root stylesheet must remain present independently of canvas visibility.

## Browser build compatibility

The overlay backend and browser WASM compile the same SHA-256 build identity from
`scripts/overlay-build.rs`. Its inputs cover overlay sources, host styles,
workspace dependencies and build configuration; this is a build identity, not the
configuration revision. Rebuild the web bundle before building the embedded backend.
Repository tasks that build the embedded backend, release artifacts, or browser/OBS
verification targets declare `overlay:web:bundle` as a prerequisite. That task owns the
completion marker and generated assets, so the embedded build receives the bundle from
the same source revision. Plain Cargo type checking does not enforce workflow-level
bundle freshness; use the repository task when producing an executable or distributable
artifact.

Each stage connection and editing request must match the backend build identity.
Until the first matching stage arrives, editing is disabled. A mismatch discards the
local draft, selection, gestures, title editing and undo history, then reloads the
workspace once to fetch the matching assets. The client records its build identity
in session storage before reloading; if the same client build mismatches again, it
stops instead of entering a reload loop and displays a manual reload fallback. It
does not save or restore the draft. Reload starts from persisted settings.
Display-only canvas URLs are not editor sessions.

The server owns each WebSocket's editor identity and releases that connection's
lease on disconnect, including mismatch rejection. Reconnecting cannot release
another connection's lease. A waiting editor retries acquisition periodically.
The private child diagnostics report `overlay_editor_version` (success or
`version_mismatch`) and `overlay_editor_connection` release outcomes; the existing
controller records lease and commit outcomes.

For upgrade verification, keep a page open while replacing the isolated test
server with another build. Confirm automatic navigation to matching assets, absence
of configuration writes from the discarded draft, and successful editing after the
new stage connects. Repeating the mismatch for one client build must stop after one
automatic reload and expose the fallback notice. Versions predating this guard
cannot automatically reload, but their unversioned editor requests are rejected by
the new backend.
