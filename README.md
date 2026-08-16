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
contract, bounded content-addressed source ingest, and replay-index validation.
It does **not** yet contain media extraction, a training pipeline, or a runnable
capture or recognition service.

The first implementation milestone is:

```text
post-scale PipeWire frame
  -> canonical RGB8 1920x1080 frame
  -> scorepeek-owned field recognizers
  -> CTC title logits scored against a federated IIDX catalog
  -> fail-closed Unix-socket NDJSON events
```

The game-session runtime will be Rust. Python is limited to reproducible,
offline OCR training and ONNX export.

## Project boundaries

- Build layout profiles from scorepeek captures; do not copy upstream code,
  coordinates, visual resources, or music data.
- Synchronize Tachi, Textage, and an official-INFINITAS-derived roster locally,
  preserving source lineage and quarantining ambiguous federation results.
- Use catalog strings only as an inference-time OCR lexicon, not as model
  training text.
- Keep real captures and labels, raw source snapshots, generated catalogs,
  models, player data, and credentials outside the repository.
- Compare Wayland Portal, Gamescope direct PipeWire, and a conditional
  post-scale OBS path on the target Bazzite machine before selecting a backend.
- Keep UI, score persistence, and external-service integration outside v1.

See [the current committed checkpoint](STATUS.md),
[the Japanese implementation plan](docs/plan.ja.md), the
[architecture overview](docs/architecture.md), the
[source policy](docs/sources.md), and [research evidence](docs/research.md).

## Development

Install [mise](https://mise.jdx.dev/), then use the repository entry points:

```text
mise trust
mise install
mise run check
mise run fix
mise run test
mise run doctor
mise run catalog:sync
mise run corpus:ingest -- --store /absolute/private/store /absolute/source.media /absolute/request.json
mise run corpus:generation:seal -- --store /absolute/private/store generation-001
mise run corpus:replay:validate -- --store /absolute/private/store /absolute/replay-suite.json
mise run catalog:schedule:systemd:verify
```

`check` is non-mutating, `fix` applies supported formatting fixes, and `test`
contains every reproducible repository check. Live Bazzite, Portal, OBS,
Gamescope, and GPU verification remains in explicit target-only tasks.

`mise run doctor` prints a versioned JSON inventory using fixed local commands
and allowlisted parsers. Missing target tools are reported as `unavailable`;
command stderr is never included. Running Gamescope flags and authenticated OBS
state remain unavailable until an exact, secret-safe probe contract exists.

`scorepeek catalog sync` acquires the catalog writer lock, resolves Tachi's
`main` branch to an exact Git commit, serially fetches the three IIDX seed JSON
collections at that commit, fetches and safely decodes the three Textage table
assignments without executing JavaScript, then fetches the dqn/iidxapi
INFINITAS roster. It applies strict status, redirect, timeout, size, encoding,
grammar, schema, digest, source-health, and federation gates before activation.
Verified source bytes are kept privately below
`$XDG_CACHE_HOME/scorepeek`, while the content-addressed catalog is stored below
`$XDG_DATA_HOME/scorepeek`; the usual `$HOME/.cache` and
`$HOME/.local/share` fallbacks apply when those variables are unset. The Tachi
cache admits at most 8 verified revisions and 512 MiB total, the Textage and
dqn caches each admit at most 64 revisions and 64 MiB total, and the catalog
store admits at most 32 snapshots, 128 MiB per snapshot and 512 MiB total. New
content fails closed when a limit is reached. The command emits revision,
digest, record count, active digest, and aggregate quarantine counts without raw
source records.

`scorepeek-corpus` is a separate workspace crate and offline binary; the
game-session `scorepeek` crate does not depend on it. Ingest requires an
explicit absolute private-store root and copies immutable source media into a
bounded SHA-256-addressed store with private permissions. Before assigning
splits, generation sealing records every current fixture binding in one
immutable content-addressed generation. Replay-suite validation reads that
generation plus the canonical source manifests and media from the store,
requires complete generation coverage, and checks source PTS plus decode
ordering, opaque fixture and episode IDs, profile separation,
extractor/annotation/frame hashes, and corpus-wide session/play/title grouped
split isolation. It emits only opaque IDs, hashes, roles, and aggregate counts.
See [the private corpus contract](docs/private-corpus.md). Media extraction and
training/export remain later offline stages and will not become game-session
runtime dependencies.

`scorepeek catalog sync` is the scheduling interface. A user may keep recurring
execution disabled and run it manually, or select any scheduler that preserves
the desired daily jitter; no schedule is enabled automatically. The standard
recommended route is a systemd user timer, with a per-run delay of up to six
hours. All routes invoke the same command and therefore share the catalog
writer lock and fail-closed exit behavior.

The scheduler deliberately does not select a catalog acquisition mode. The
current command always builds from validated sources on the host. If a future
source-policy and ADR decision permits GitHub-managed catalog distribution, the
same command boundary will allow a user to choose local self-build or verified
provided-catalog acquisition, and GitHub scheduling will run the self-build
orchestration. No provided-catalog path is enabled today.

The systemd installer builds a locked release binary at
`%h/.local/bin/scorepeek`, installs the user units below
`$XDG_CONFIG_HOME/systemd/user` (or `$HOME/.config/systemd/user`), and enables
the timer only when explicitly invoked:

```text
mise run catalog:schedule:systemd:install
```

The installed service records the current absolute `$XDG_DATA_HOME` and
`$XDG_CACHE_HOME` values, or their standard home-directory fallbacks, so later
scheduled runs use the same catalog roots as the installing shell. Re-run the
installer after intentionally changing those roots.

The persistent host deployment is explicit and is not performed by
`mise run test`; its non-mutating unit verification is included.
All scorepeek systemd scheduling can be stopped, and the installed timer can be
disabled without removing the binary or unit files, with
`mise run catalog:schedule:systemd:disable`. This also stops a running transient
timer, allowing an immediate return to manual-only synchronization.
Persistent and transient timers are mutually exclusive: either activation task
fails closed and points to this disable task when the other mode is active or
enabled.

To keep a daily timer only for the lifetime of the current systemd user manager
without installing unit files or enabling it across restarts, use:

```text
mise run catalog:schedule:systemd:start:transient
```

This transient route runs the locked release binary from the current repository
and deliberately sets `Persistent=false`; moving the repository invalidates
that transient service. Keeping recurring execution disabled requires no setup
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
