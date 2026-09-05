# scorepeek

`scorepeek` is a private, Linux-first companion that turns IIDX game screens
into structured recognition events. It is an independent implementation: the
Windows application that inspired the project is neither a Git parent nor a
runtime, catalog, resource, or release input.

## Status

The repository currently contains the accepted design, research evidence,
validation scaffold, a Rust target-inventory probe, and the first catalog-core
slice: strict synthetic fixture adapters, deterministic fail-closed federation,
quarantine results, durable content-addressed local snapshots, and bounded live
Tachi, Textage, and dqn/iidxapi acquisition through `scorepeek catalog sync`.
It also contains an opt-in daily systemd user schedule for that same command.
The offline-only `scorepeek-corpus` tool now owns the first private-corpus
contract, bounded content-addressed source ingest, canonical complete-label
authoring, deterministic episode/replay-index generation, a seed-only
procedural synthetic title renderer, and replay-index validation.
It also pins an offline static FFmpeg/ffprobe toolchain for bounded private
media probing and explicit observed RGB8 frame extraction. Complete recording
runs can be imported as reusable dataset roots, sealed by digest, and
explicitly synchronized with private S3-compatible storage. Shared canonical
normalization, a measured result layout, and a locked offline PP-OCRv6 field
spike are implemented for one exact OBS/vkcapture profile and recording. The
same title crop now passes a diagnostic Paddle/official-ONNX/Rust parity gate
for the complete preprocessed input, exported CTC probability tensor, and token
order. Rust also scores every exactly encodable non-search title in one active
catalog through a shared CTC trie and fails closed against explicit absolute and
runner-up thresholds or incomplete registered-dictionary coverage. A supported
capture route, training/export pipeline, and runnable recognition service are
not yet implemented.

The first implementation milestone is:

```text
opaque capture-profile frame
  -> versioned domain normalizer
  -> conceptual canonical RGB8 1920x1080 frame
  -> scorepeek-owned field recognizers
  -> CTC title logits scored against a federated IIDX catalog
  -> fail-closed Unix-socket NDJSON events
```

The game-session runtime will be Rust. Python is limited to reproducible,
offline OCR tooling: the current Paddle inference spike, later training, and
ONNX export.

## Project boundaries

- Own one game layout in the canonical frame contract and calibrate each
  capture profile to it; do not copy upstream code, coordinates, visual
  resources, or music data.
- Synchronize Tachi, Textage, and an official-INFINITAS-derived roster locally,
  preserving source lineage and quarantining ambiguous federation results.
- Use catalog strings only as an inference-time OCR lexicon, not as model
  training text.
- Keep real captures and labels, raw source snapshots, generated catalogs,
  models, player data, and credentials outside the repository.
- Validate Wayland Portal, Gamescope direct PipeWire, and a conditional OBS
  profile independently on the target Bazzite machine before selecting a
  default; none is a pixel correctness reference.
- Persist local scores as an independent event consumer; keep UI and external-service integration outside v1.

See [the current committed checkpoint](STATUS.md),
[the Japanese implementation plan](docs/plan.ja.md), the
[architecture overview](docs/architecture.md), the
[source policy](docs/sources.md), and [research evidence](docs/research.md).

## Local distribution

Cargo-dist 0.32.0 builds the ordinary Linux x86-64 CLI archive locally. This repository does not
publish a GitHub Release, tag, installer or source archive.

```text
mise run dist:plan
mise run dist:build
mise run dist:test
```

The build writes `target/distrib/scorepeek-x86_64-unknown-linux-gnu.tar.xz` and its `.sha256`
sidecar. Verify the checksum, extract the archive and copy the executable to the usual user-local
binary directory:

```text
cd target/distrib
sha256sum --check scorepeek-x86_64-unknown-linux-gnu.tar.xz.sha256
tar -xJf scorepeek-x86_64-unknown-linux-gnu.tar.xz
install -Dm755 scorepeek-x86_64-unknown-linux-gnu/scorepeek "$HOME/.local/bin/scorepeek"
scorepeek --version
scorepeek doctor
```

The archive does not contain private catalogs, OCR models, capture bindings, frames or credentials.
Catalogs remain separately managed operator data under `$XDG_DATA_HOME/scorepeek` (normally
`$HOME/.local/share/scorepeek`). The fixed PP-OCRv6-small model is different: the first ordinary
command downloads its three registered files from the immutable official revision and publishes
them below `$XDG_CACHE_HOME/scorepeek/models` (normally `$HOME/.cache/scorepeek/models`). The
registered source is Apache-2.0. `--help`, `--version`, and `doctor` do not initialize the model.
Deleting the cache is safe; the next ordinary command downloads it again and therefore needs a
network connection. For offline use, successfully run one ordinary command while online first.
The release task itself does not acquire the model or include it in the archive.
Developers may instead provide a complete fixed small bundle with
`scorepeek --model-bundle /absolute/directory <command...>`; scorepeek verifies the same registered
contract and does not use the network. This is not an alternate-model selector.

RESULT and MUSIC SELECT digits require the separately frozen private numeric bundle. The current
registration uses the SELECT-adapted HOG/MLP weights (ADR 0115); older bundles do not satisfy it.
Install its create-only,
digest-bound model before `run` with
`scorepeek numeric-model install --bundle /absolute/numeric-model-bundle`. There is no general-text
numeric fallback: a missing or mismatched active bundle makes `run` fail closed. `scorepeek doctor`
uses `scorepeek-doctor-v2`; it reports the unchanged `scorepeek-target-inventory-v1` under
`target_inventory` and the active numeric model identity or typed unavailability under
`numeric_model`.

After transferring or synchronizing one active catalog, create a capture profile on the machine
that will run the game. Scorepeek starts and stops a dedicated calibration Gamescope containing its
own marker; arguments after `--` are used only to launch that calibration process:

```text
scorepeek setup gamescope --profile bazzite-4k -- -W 3840 -H 2160 -w 1920 -h 1080 -r 120 -S fit -F linear
scorepeek profile list
```

Setup stores the profile under `$XDG_CONFIG_HOME/scorepeek/profiles` (normally
`$HOME/.config/scorepeek/profiles`). Operator-selected local roots and inputs follow normal
filesystem symlinks, including Bazzite's `/home -> /var/home`; resolved files still have to satisfy
their type, size, digest, schema, and admission contracts. Setup measures the positive
axis-aligned X/Y scale and translation from the captured marker and saves only the observed BGRx
dimensions and rational source rectangle needed by the production normalizer. Padding,
non-centered and fractional offsets, non-integer or different X/Y scales, and aspect distortion
are accepted when every canonical pixel-center sample is present; a signed half-pixel source
origin can represent normal scaler phase. Crop, rotation, mirror, shear,
perspective, or unreadable marker interiors are rejected because the current normalizer cannot
recover them. Gamescope version, backend, filter, scaler, refresh, launch arguments, stride, and
memory allocation are not profile identity.

Setup does not start INFINITAS and proves only the capture transform; it does not turn an
unverified configuration into a supported profile. Existing local profile schemas must be
recreated with setup. Start the watcher before or after the ordinary Gamescope/game session:

```text
scorepeek run --profile bazzite-4k --record
```

The default shared recording-memory limit is 1024 MiB. Override it for a recorded invocation with:

```text
scorepeek run --profile bazzite-4k --record --record-memory-mib 2048
```

When exactly one profile exists, `scorepeek run` selects it automatically. Multiple profiles require
`--profile NAME`. Recording is disabled by default; add `--record` to retain structured watcher,
diagnostic, recognition, event, and canonical replay artifacts. Routine recording does not retain
legacy QOI images beside the canonical segments. The TUI reports recording memory usage and marks
the session `degraded` immediately after a recording loss; `recording_ready` means the atomically
published session can be imported even while the watcher continues running. The watcher waits when no source
exists, attaches only when exactly one Gamescope video source exists, and stays running across
sequential Gamescope lifetimes. A unique startup source that is not ready for admission is retried at
a bounded interval, so scorepeek can remain running while Gamescope and the game finish starting.
Stop scorepeek with SIGINT (normally Ctrl-C) or SIGTERM. Scorepeek
does not start, signal, stop, or restart ordinary Gamescope, Steam, or INFINITAS processes.
Each Gamescope session is admitted from the actual source format, dimensions, current byte layout,
and saved geometry. Music-select/result scene detection and OCR during ordinary `run` are the
authority for recognition support; scorepeek does not re-estimate geometry or switch profiles at
runtime.

On a terminal, `run` shows Watcher, Latest result, Music Select Resolver, and RESULT/attempt
Resolver panes. Latest result prefers the current provisional payload and otherwise shows the last
confirmed result; only confirmed results enter the count/history. The Music Select Resolver shows
selected chart identity, self-best SCORE/MISS/clear values, per-field `1/2` stabilization, and the
snapshot output gate/revision. Missing identity evidence retains the interval as `held`, blocks
value adoption and restarts field stabilization. UNKNOWN suspends the retained interval. Contrary
song/mode/difficulty evidence clears values; SELECT exit returns it to inactive. Recovery with
identical values does not re-emit a snapshot. The 80x25 layout keeps existing attempt gates visible.
DJ rank is calculated from EX SCORE and chart notes, not recognized from the screen.

Machine consumers connect to `$XDG_RUNTIME_DIR/scorepeek/v1.sock` for an initial
`scorepeek-event-snapshot-v1` followed by `scorepeek-event-v1` NDJSON. It publishes confirmed and
provisional RESULTs, current selection, supplemental SELECT best, and operational status. Raw OCR
and resolver diagnostics stay internal and in opt-in recordings. The old observation socket is removed.
Reconnection restores current state; this live API does not recover every missed play or implement
history replay. An independent in-process consumer saves scores without requiring a socket connection.
Redirected stdout remains deduplicated human-readable status.
See [Event API v1](docs/event-api.md) for wire fields, consumer state and delivery limits, and
[the SELECT best decision](docs/decisions/0114-observe-music-select-best-snapshots.md).

`run` saves confirmed plays and chart bests to `$XDG_DATA_HOME/scorepeek/scores.sqlite3`
(defaulting to `$HOME/.local/share/scorepeek/scores.sqlite3`). Use `--scores-db PATH` to select another
DB, including a guest DB, or `--no-scores` to disable saving. These options are independent of
`--record`; switching DBs requires restarting run. A selected DB is never silently replaced by the default.

SELECT-only charts are saved without creating plays. SELECT retains only the latest known supplement
per field, allowing later observations to correct it; unknown/not-displayed leave it unchanged.
Explicit no-record clears that supplemental field. Combined bests use RESULT history, its previous-best
values and current SELECT supplements. Guest DBs still receive the current game account's supplements.
Save failure is shown as degraded while recognition continues; uncommitted data is not recoverable
after a crash. See [the score database contract](docs/decisions/0120-persist-scores-as-event-consumer.md)
for schema, source attribution, failure limits and timestamp semantics. No history query CLI is included.

### Live overlays

The release executable embeds both renderers. Enable either or both and optionally select the
strict TOML document:

```sh
scorepeek run --overlay-wayland --overlay-obs --overlay-config ./overlay.toml
```

Without `--overlay-config`, the document is `$XDG_CONFIG_HOME/scorepeek/overlay.toml` (or the
standard HOME fallback). The first run creates one enabled 560x1040 canvas for each backend with
status, selection, score, history-list and history-graph widgets. OBS serves stable canvas URLs such
as `http://127.0.0.1:3939/canvas/obs-main`; configure that URL once as a Browser Source. The listen
address is `obs_listen` in TOML. Omit both enable flags for an overlay-free run.

Right-click anywhere on the Wayland canvas or the OBS page in Browser Source Interaction to enter
its editor; DONE is the only exit. Drag/resize widgets, add or remove them, switch among CYAN SYSTEM,
RESULT AURORA and DJ BLACKBOX, and configure history rows or graph range. OBS also manages its canvas
list there. Wayland normally uses left-drag to move a canvas, keeps at least 32 pixels visible on its
selected output, and recreates a surface when its output selection changes. The initial native canvas
uses a 20-pixel upper-right inset. All edits pass through the parent and are atomically saved to TOML.

Selection shows title/artist plus a separate SP/DP, difficulty, level and notes rail. Score, recorded
state, RESULT detail and history come only from committed SQLite readback. History dates use local
notification time. The graph labels DJ LEVEL thresholds and uses a fixed 0-100% MISS RATE axis.
`--no-scores` disables those DB-derived values. Overlay failure does not stop recognition or saving.
See [the canvas contract](docs/decisions/0125-compose-overlay-canvases-from-widgets.md) and
[RESULT ingest lifecycle](docs/decisions/0126-publish-result-ingest-lifecycle.md).

Development: `mise run overlay:web:check` needs no bundle; `mise run overlay:web:test` builds and
checks real assets and owned-child cleanup. `mise run dist:build` includes Oxanium, its OFL license,
three skin frames and the browser bundle in the single binary. `mise run overlay:test:live
--scores-db PATH` is an explicit desktop gate requiring a dedicated test database. GUI and target
performance verification remain separate from unit tests.

and leaves `mise run catalog:sync` as the manual route.

`mise run catalog:schedule:systemd:verify` can also perform that non-mutating
unit validation independently. `mise run catalog:schedule:systemd:test:live` is an explicit
networked live gate: it uses private temporary XDG roots and a transient
one-second timer, starts a manual sync while the scheduled run holds the writer
lock, verifies both aggregate-only invocations succeed, and removes the
acquired bytes and generated catalog afterward. Output equality is reported but
is not required because an upstream source may legitimately change between the
serialized acquisitions. It does not install or enable the persistent timer.

## Licensing

No public license or redistribution grant is asserted. Development is private,
and every external source, font, model, and runtime artifact must retain its
provenance, immutable revision, digest, and applicable license or permission.
Third-party data is fetched locally and is not republished from this repository.

### Overlay skins

CYAN SYSTEM, RESULT AURORA and DJ BLACKBOX are stored per canvas in TOML and can be changed from the
canvas editor. The native and browser renderers share the same semantic DOM and CSS skin variables.
Approved generated frame and aurora artwork is embedded as CSS backgrounds; live text and SVG/chart
values are never baked into those images. Oxanium is embedded for Latin/numeric text and Japanese
uses system-font fallback. See the [approved design masters](docs/design/overlay-canvas/README.md).
