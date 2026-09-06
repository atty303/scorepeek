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
