# scorepeek skin plugin API v1

This is the language-neutral authoring contract for `api_version = 1`. Before scorepeek's first
public distribution, v1 may change in place as recorded by ADR 0143.

## ZIP layout

The archive contains one skin and normalized relative paths. Root files `skin.toml`, `skin.wasm`,
`skin.css`, and `preview.png` are mandatory. `preview.webm` is optional. Other files are
package-relative resources. CSS `url('background.png')` addresses the ZIP root; `../`, absolute
paths, remote URLs, filesystem, and network host calls are not package APIs.

`skin.toml` has this shape. `id` is a lowercase ASCII reverse-domain name, `release` is an opaque
non-empty string, and property keys use lowercase ASCII letters, digits, and `-`. Properties may be
omitted entirely; the example shows every supported type:

```toml
id = "dev.example.skin"
name = "Example"
release = "2026-09-08"
api_version = 1
description = "Optional description"
author = "Optional author"
license = "Optional SPDX expression or label"
homepage = "https://example.invalid/skin"

[canvas_properties.enabled]
type = "boolean"
default = true

[canvas_properties.mode]
type = "enum"
default = "quiet"
values = ["quiet", "bright"]

[widget_properties."*".count]
type = "integer"
default = 1
minimum = 0
maximum = 10

[widget_properties."*".intensity]
type = "number"
default = 0.5
minimum = 0.0
maximum = 1.0

[widget_properties."*".accent]
type = "color"
default = "#42d9ff"

[widget_properties."*".label]
type = "string"
default = ""
maximum_length = 80
```

An enum must contain distinct non-empty values and its default must be one of them. Integer and
number bounds are inclusive and must contain the default; numbers must be finite. Colors are
`#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA`. A widget property table names a widget kind or uses `*`
as its fallback. The editor exposes only declared properties.

## Core ABI

The module imports nothing and exports `memory` plus:

```text
scorepeek_alloc(length: i32) -> i32
scorepeek_dealloc(pointer: i32, length: i32)
scorepeek_init(pointer: i32, length: i32) -> i64
scorepeek_render(pointer: i32, length: i32) -> i64
```

Input is UTF-8 JSON written to the allocation. Output `i64` packs its pointer in the high 32 bits
and byte length in the low 32 bits. Output remains readable until the next call. Calls are
synchronous and the host deallocates each successful input. There is no WASI or host import.

Input schema `scorepeek-skin-input-v1` contains `backend` (`native` or `obs`), canvas identity,
dimensions and effective properties, host widget geometry/settings/effective properties, and the
versioned semantic presentation snapshot. Unknown additive input fields must be ignored.

The complete input shape is:

```json
{
  "schema": "scorepeek-skin-input-v1",
  "backend": "native",
  "canvas": {"id": "main", "skin": "dev.example.skin", "width": 1920, "height": 1080, "properties": {}},
  "widgets": [{
    "id": "score", "kind": "score", "x": 20, "y": 20,
    "width": 640, "height": 280, "settings": {}, "properties": {}
  }],
  "state": {
    "connected": true,
    "chart": {
      "song_id": "example", "play_type": "SP", "difficulty": "ANOTHER",
      "title": "Example", "artist": "Artist", "level": 12, "notes": 1000
    },
    "system": "active", "result_signal": "inactive",
    "best": {"score": "1778", "dj_level": "AAA", "miss": "0", "clear": "FULL COMBO"},
    "detail": {
      "pgreat": "800", "great": "178", "good": "10", "bad": "4", "poor": "8",
      "fast": "12", "slow": "9", "combo_break": "3", "play_options": "RANDOM"
    },
    "history": {
      "recorded": true,
      "plays": [{"notified_at": "2026-09-08", "score": "1778", "dj_level": "AAA", "miss": "0", "clear": "FULL COMBO"}],
      "graph": [{"received_unix_ms": 1788825600000, "score_ratio": 0.889, "miss_ratio": 0.0}],
      "graph_ticks": [{"unix_ms": 1788825600000, "label": "09/08"}],
      "graph_start_unix_ms": [1788825600000, 1788825600000, 1788825600000, 1788825600000],
      "graph_end_unix_ms": 1788825600000
    },
    "screen": {"kind": "result", "suspended_since_unix_ms": null, "revision": 1}
  }
}
```

`chart` and its `level`/`notes` may be `null`; `history.graph[].miss_ratio`, `screen.kind`, and
`screen.suspended_since_unix_ms` may also be `null`. `system` and `result_signal` are `inactive`,
`active`, or `error`. Screen kinds are `music-select`, `mode-select`, `decide-transition`, `play`,
and `result`. Presentation strings may be empty when the value is unavailable; plugins must not
recompute game meaning from them.

Output contains `schedule` and one complete `tree`. Schedule kinds are `idle`, `next-frame`, and
`after-ms` with `milliseconds` in the inclusive range `0..=2147483647`; larger values are
rejected identically by native and OBS. Nodes are ordinary DOM data:

```json
{"kind":"element","key":"canvas","tag":"main","attributes":{},"children":[]}
{"kind":"text","key":"label","text":"value"}
```

Every key is non-empty and unique. The host uses keys to reconcile full trees. Plugins receive no
pointer, keyboard, focus, click, timer, thread, filesystem, network, database, Event API fold, or
asynchronous callback interface.

Rust reference types and buffer helpers live in `crates/scorepeek-skin-sdk`. The repository guest
in `skins/guest-core` is the buildable reference used by the three repository skins; it reproduces
their complete widget hierarchy, image-backed materials, atlas typography, graphs, and motion.

## Local package management

Build the repository examples with `mise run overlay:skins:build`; ZIPs are written below
`target/skins/` and are not installed automatically. Manage any local ZIP with:

```text
scorepeek skin install PATH.zip
scorepeek skin list
scorepeek skin uninstall dev.example.skin-name
```

The store is `$XDG_DATA_HOME/scorepeek/skins` (or the corresponding home data directory). A package
whose ID and release already match is reported as `unchanged` without replacing the stored ZIP.
Changing the opaque release replaces that ID atomically after the v1 property compatibility check.
Uninstall does not rewrite canvases that reference the ID; install or select an available skin
before the next overlay startup. There is no live reload or protection for a package changed while
an overlay process is using it.
