# scorepeek

`scorepeek` is a private, Linux-first companion that turns IIDX game screens
into structured recognition events, local score history, and live overlays. It
is an independent implementation: the Windows application that inspired the
project is neither a Git parent nor a runtime, catalog, resource, or release
input.

## Current capabilities

The current Rust runtime:

- synchronizes and fail-closed federates Tachi, Textage, and dqn/iidxapi
  catalog data;
- creates machine-local Gamescope capture profiles and receives Gamescope
  direct PipeWire frames;
- normalizes admitted BGRx frames to contiguous RGB8 1920x1080;
- recognizes semantic screens, song/chart identity, RESULT performance,
  play options, and MUSIC SELECT self-best values with registered text and
  numeric models;
- publishes Event API v2 snapshots and ordered NDJSON on
  `$XDG_RUNTIME_DIR/scorepeek/events.sock`;
- persists provisional, retracted, and confirmed results plus SELECT
  supplements in SQLite;
- renders independent Wayland and OBS overlays from installable Wasm/CSS skin
  packages; and
- optionally records bounded diagnostics and canonical sessions for private
  corpus replay.

The Gamescope runtime, recognition, score persistence, and overlay paths have
also been exercised on the target machine. Browser integration, fake Wayland,
and the checked-in nested compositor scenario remain the routine reproducible
overlay gates.

The game-session runtime is Rust. Python is limited to reproducible offline OCR
preparation, training, and ONNX export tooling.

## Project boundaries

- Own the canonical game layout and independently measure every committed
  coordinate from scorepeek captures. Do not copy upstream code, coordinates,
  visual resources, catalogs, or generated artifacts.
- Calibrate the Gamescope capture profile explicitly. Runtime recognition does
  not remeasure geometry, relax thresholds, switch profiles, or fall back to a
  different capture route.
- Preserve source lineage and quarantine ambiguous catalog federation results.
- Use external catalog strings only as runtime decoder input, not as OCR
  training text.
- Keep real captures and complete labels, raw source snapshots, generated
  catalogs, private models, player data, and credentials outside the
  repository, except for explicitly approved synthetic or narrow template
  fixtures.
- Fail closed: missing resources, ambiguous evidence, schema drift, or
  unsupported input produce typed unavailability rather than guessed values.

See the [architecture map](docs/architecture.md),
[field semantics](docs/field-semantics.md),
[Event API v2](docs/event-api.md), and
[external source policy](docs/sources.md).

## Local distribution

Cargo-dist 0.32.0 builds the Linux x86-64 CLI archive locally. This repository
does not publish a GitHub Release, tag, installer, or source archive.

```text
mise run dist:plan
mise run dist:build
mise run dist:test
```

The build writes `target/distrib/scorepeek-x86_64-unknown-linux-gnu.tar.xz`
and its `.sha256` sidecar:

```text
cd target/distrib
sha256sum --check scorepeek-x86_64-unknown-linux-gnu.tar.xz.sha256
tar -xJf scorepeek-x86_64-unknown-linux-gnu.tar.xz
install -Dm755 scorepeek-x86_64-unknown-linux-gnu/scorepeek "$HOME/.local/bin/scorepeek"
scorepeek --version
scorepeek doctor
```

The archive does not contain catalogs, OCR models, capture profiles, frames,
scores, or credentials.

## Catalog and models

`scorepeek catalog sync` acquires source data, federates it, and atomically
activates a valid snapshot under `$XDG_DATA_HOME/scorepeek` (normally
`$HOME/.local/share/scorepeek`). A failed sync leaves the last-known-good
catalog active.

```text
scorepeek catalog sync
```

The fixed registered PP-OCRv6-small files are fetched from their immutable
official revision into `$XDG_CACHE_HOME/scorepeek/models` during common CLI
initialization. `--help`, `--version`, `doctor`, `numeric-model install`, and
`skin install`, `skin list`, or `skin uninstall` bypass that initialization;
other commands initialize the bundle before dispatch even when they do not
perform OCR themselves. For offline use, run one of those initializing commands
while online first. Developers may provide the same complete registered bundle with
`scorepeek --model-bundle /absolute/directory COMMAND ...`; this is not an
alternate-model selector.

RESULT and MUSIC SELECT digits use the installed private fixed-slot HOG/MLP
numeric bundle. Install it create-only before `run`:

```text
scorepeek numeric-model install --bundle /absolute/numeric-model-bundle
```

A missing or mismatched active numeric bundle makes recognition fail closed;
the text recognizer is not a numeric fallback. `scorepeek doctor` reports the
target inventory and active numeric model identity or typed unavailability.

The optional user-systemd schedule invokes the same catalog sync operation:

```text
mise run catalog:schedule:systemd:verify
mise run catalog:schedule:systemd:install
```

Installation and enabling are explicit operations. The live schedule test uses
isolated temporary XDG roots and removes its acquired data:

```text
mise run catalog:schedule:systemd:test:live
```

## Gamescope setup and run

Create a capture profile on the machine that runs the game. Scorepeek starts
and stops only a dedicated calibration Gamescope containing its own marker;
arguments after `--` belong to that calibration process.

```text
scorepeek setup gamescope --profile bazzite-4k -- -W 3840 -H 2160 -w 1920 -h 1080 -r 120 -S fit -F linear
scorepeek profile list
```

Profiles are stored below `$XDG_CONFIG_HOME/scorepeek/profiles` (normally
`$HOME/.config/scorepeek/profiles`). Setup measures positive axis-aligned X/Y
scale and translation and stores the observed BGRx dimensions and rational
source rectangle. Padding, non-centered or fractional offsets, unequal X/Y
scales, and aspect distortion are accepted when every canonical sample is
present. Crop, rotation, mirror, shear, perspective, and unreadable marker
interiors are rejected.

Start scorepeek before or after the ordinary Gamescope/game session:

```text
scorepeek run --profile bazzite-4k
```

When exactly one profile exists, `--profile` may be omitted. Scorepeek waits
for exactly one Gamescope video source and stays alive across sequential source
lifetimes. It does not start, stop, signal, or restart the operator's ordinary
Gamescope, Steam, or game processes.

Recording is disabled by default. Add `--record` to retain structured
capture, recognition, event, and canonical replay artifacts. The shared
recording-memory limit defaults to 1024 MiB:

```text
scorepeek run --profile bazzite-4k --record --record-memory-mib 2048
```

Recording loss marks evidence degraded but does not change recognition,
events, or score persistence. See [private corpus](docs/private-corpus.md) and
[diagnostic controls](docs/diagnostic-controls.md).

## Events and scores

On a terminal, `run` shows Watcher, Latest result, Music Select Resolver, and
RESULT/attempt Resolver panes. Latest result explicitly shows inactive,
provisional, retracted, or confirmed state; only confirmed results enter the
process count/history.

Machine consumers receive one `scorepeek-event-snapshot-v2` followed by
`scorepeek-event-v2` records from
`$XDG_RUNTIME_DIR/scorepeek/events.sock`. Reconnection restores current
state, not a complete event log. Raw OCR, candidates, resolver metrics, and
recording paths remain internal. See [Event API v2](docs/event-api.md).

The independent in-process score consumer writes
`$XDG_DATA_HOME/scorepeek/scores.sqlite3` by default. Select another database
or disable persistence per invocation:

```text
scorepeek run --scores-db /absolute/guest.sqlite3
scorepeek run --no-scores
```

Provisional RESULTs are saved immediately, retractions delete the same
attempt, and RESULT finalization confirms it. SELECT-only charts may hold
current per-field supplements without creating plays. Save failure degrades
score health while recognition and Event API delivery continue.

## Live overlays

Enable either or both renderers and optionally select the strict schema-v8 TOML
document:

```text
scorepeek run --overlay-wayland --overlay-obs --overlay-config /absolute/overlay.toml
```

Without `--overlay-config`, the document is
`$XDG_CONFIG_HOME/scorepeek/overlay.toml`. A missing document starts with
empty Wayland and OBS workspaces; the editor creates canvases. For OBS, use
`http://127.0.0.1:3939/overlay` for the complete workspace or
`http://127.0.0.1:3939/canvas/CANVAS_ID` for one canvas. `obs_listen`
controls the listen address.

Right-click the Wayland stage or OBS page in Browser Source Interaction to
enter the shared editor. Canvas movement uses secondary-button drag; widget
movement and resize use the primary button. Each canvas has an explicit output,
name, screen visibility, opacity, geometry, widgets, and installed skin.
Disconnected outputs and invalid geometry remain explicit editor errors until
the operator reassigns or fits them. The configuration loader accepts only the
current schema-v8 document.

Score, recorded state, RESULT detail, history, and graphs come only from
committed SQLite readback. Overlay failure does not stop recognition, score
persistence, or the other backend. Canvas DOM, CSS, and resources come from
the selected skin package; the host owns canvas/widget geometry and semantic
input. See [skin plugin API v1](docs/skin-plugin-api-v1.md) and
[overlay visual debugging](docs/overlay-visual-debugging.md).

Build and install the repository skins explicitly:

```text
mise run overlay:skins:build
scorepeek skin install target/skins/cyan-system.zip
scorepeek skin list
```

Skin ZIPs are not embedded in the executable.

## Licensing

No public license or redistribution grant is asserted. Development is private,
and every external source, font, model, and runtime artifact must retain its
provenance, immutable revision, digest, and applicable license or permission.
Third-party data is fetched locally and is not republished from this repository.
