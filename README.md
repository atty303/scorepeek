# scorepeek

`scorepeek` is a private, Linux-first companion that turns IIDX game screens
into structured recognition events. It is an independent implementation: the
Windows application that inspired the project is neither a Git parent nor a
runtime, catalog, resource, or release input.

## Status

The repository currently contains the accepted design, research evidence,
validation scaffold, and a Rust target-inventory probe. It does **not** yet
contain a runnable capture or recognition service.

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

See [the Japanese implementation plan](docs/plan.ja.md), the
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
```

`check` is non-mutating, `fix` applies supported formatting fixes, and `test`
contains every reproducible repository check. Live Bazzite, Portal, OBS,
Gamescope, and GPU verification remains in explicit target-only tasks.

`mise run doctor` prints a versioned JSON inventory using fixed local commands
and allowlisted parsers. Missing target tools are reported as `unavailable`;
command stderr is never included. Running Gamescope flags and authenticated OBS
state remain unavailable until an exact, secret-safe probe contract exists.

## Licensing

No public license or redistribution grant is asserted. Development is private,
and every external source, font, model, and runtime artifact must retain its
provenance, immutable revision, digest, and applicable license or permission.
Third-party data is fetched locally and is not republished from this repository.
