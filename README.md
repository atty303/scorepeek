<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/scorepeek-logo-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/scorepeek-logo-light.png">
    <img src="docs/assets/scorepeek-logo-light.png" alt="scorepeek" width="760">
  </picture>
</p>

<p align="center">
  An IIDX companion that automatically records your results and brings your progress into view.
</p>

| Cyan System | Result Aurora | DJ Blackbox |
| :---: | :---: | :---: |
| ![Cyan System overlay preview](skins/cyan-system/preview.png) | ![Result Aurora overlay preview](skins/result-aurora/preview.png) | ![DJ Blackbox overlay preview](skins/dj-blackbox/preview.png) |

Three examples of how your overlay can look. Choose a skin, arrange your widgets,
or create a look of your own.

## What you get

### Automatic score tracking

Keep a local record of your play results without entering scores by hand. See
your personal bests, recent results, history, and progress graphs in the overlay.

### Screen recognition that keeps up

Scorepeek reads your game screen using image recognition, without analyzing the
game's internal data. It matches OCR readings against song catalogs fetched
online. New songs do not need their own set of training images, reducing the
upkeep needed as the catalogs grow.

The official catalog is distributed from the project's GitHub Pages site and
updated automatically. `scorepeek run` downloads it on first use, then checks
in the background after 24 hours while continuing to use the catalog selected
at invocation start. A custom ZIP URL can be set in
`$XDG_CONFIG_HOME/scorepeek/config.toml`:

```toml
[catalog]
url = "https://example.invalid/catalog/v1/catalog.zip"
```

`SCOREPEEK_CATALOG_URL` is a temporary higher-priority override. HTTPS,
loopback HTTP for development, and `file://` artifacts are supported.

## Command line

Running `scorepeek` without arguments prints top-level help and succeeds. The public commands are
`run`, `doctor`, `config`, `diagnostic`, `skin`, `vulkan-layer`, and `completion`.

```text
scorepeek run --capture pipewire --node-name gamescope
scorepeek run --capture vulkan-layer
scorepeek doctor
scorepeek doctor --format json
scorepeek vulkan-layer install
scorepeek vulkan-layer uninstall
scorepeek config path
scorepeek config show
scorepeek config check
scorepeek diagnostic observe
scorepeek diagnostic inspect --latest
scorepeek diagnostic inspect --latest --format json
scorepeek skin list
scorepeek completion nushell
```

`scorepeek vulkan-layer install` installs the explicit capture layer embedded in this exact
Scorepeek binary. It atomically updates
`$XDG_DATA_HOME/vulkan/explicit_layer.d/VkLayer_SCOREPEEK_capture.json` and
`$XDG_DATA_HOME/scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so`; when `XDG_DATA_HOME` is
unset, `$HOME/.local/share` is used. The manifest refers to the library by a relative path, so the
Vulkan Loader discovers it without `VK_LAYER_PATH`. Installation and updates occur only when this
command is run. `scorepeek run` never changes the installed layer.

The layer remains explicit. Launch the game through the operator's existing Gamescope, Steam,
Proton, or umu configuration with `VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture`. Pressure Vessel
must still be configured there to expose `$XDG_RUNTIME_DIR/scorepeek`. Scorepeek does not launch or
configure those programs. `scorepeek vulkan-layer uninstall` removes only the Scorepeek manifest
and library; it leaves the shared Vulkan manifest directory in place. `scorepeek doctor` reports
whether the two installed files are absent, match this binary's embedded SHA-256 payload, differ
from it, or form an invalid installation.

The optional config file defaults to `$XDG_CONFIG_HOME/scorepeek/config.toml` (or
`$HOME/.config/scorepeek/config.toml`). Scorepeek never creates it. `--config FILE` selects a
different file, and `SCOREPEEK_CONFIG` selects one when the CLI option is absent. `config show`
prints only the file content; it does not show merged environment or CLI values.

```toml
[capture]
backend = "pipewire"
node_name = "gamescope"

[crop]
left = 0
top = 0
right = 0
bottom = 0

[scores]
enabled = true
database = "/absolute/path/to/scores.sqlite3"

[overlay]
wayland = true
wayland_edit = false
obs = false
config = "/absolute/path/to/overlay.toml"

[recording]
enabled = true
memory_mib = 2048

[catalog]
url = "https://example.invalid/catalog/v1/catalog.zip"
```

Run settings are merged in the order `config < SCOREPEEK_* environment < CLI`. The supported run
environment variables are `SCOREPEEK_CAPTURE`, `SCOREPEEK_PIPEWIRE_NODE_NAME`,
`SCOREPEEK_CROP_LEFT`, `SCOREPEEK_CROP_TOP`, `SCOREPEEK_CROP_RIGHT`,
`SCOREPEEK_CROP_BOTTOM`, `SCOREPEEK_SCORES_ENABLED`, `SCOREPEEK_SCORES_DB`,
`SCOREPEEK_OVERLAY_WAYLAND`, `SCOREPEEK_OVERLAY_WAYLAND_EDIT`, `SCOREPEEK_OVERLAY_OBS`,
`SCOREPEEK_OVERLAY_CONFIG`, `SCOREPEEK_RECORDING_ENABLED`, and
`SCOREPEEK_RECORDING_MEMORY_MIB`; catalog URL continues to use `SCOREPEEK_CATALOG_URL`. Selecting a
capture backend at a higher-precedence layer clears the lower layer's backend-specific settings,
so a PipeWire node name is never inherited by `vulkan-layer`.

Normal query output is human-readable. `doctor`, `config path`, `config show`, `config check`, and
`skin list` accept `--format json`, as does `diagnostic inspect`. `diagnostic observe` remains
streaming NDJSON.

### Local processing, local records

Recognition runs on your machine, and your scores are saved locally. No cloud
OCR or screen uploads are needed to recognize and record your results.

### Build your own tools

The recognition core is separated from capture and presentation for portability
and reuse. Use the Event API for live recognition events and SQLite for recorded
scores and history to build your own dashboards, analysis tools, or integrations.
The included overlays are optional.

### For your screen and your stream

Show an overlay on your own screen, in an OBS broadcast, or both. It is just as
useful for everyday play when you are not streaming.

### Make the overlay yours

Choose the information you want to see, move and resize widgets in the visual
editor, and decide which game screens show them. Install skins or create your
own; the three previews above are examples, not a fixed set of styles.

## Project status

Scorepeek is in active development and is not yet ready for distribution or
general use. The current application targets Linux.

Development is private. No public license or redistribution permission has been
granted. Third-party data and assets remain subject to their own terms.
