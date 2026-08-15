# scorepeek

`scorepeek` is a private, Linux-first companion application that turns rhythm
game screens into structured recognition events. It is intentionally developed
as an independent repository: the upstream Windows application is an external
release/resource input, not a Git parent and not a runtime dependency.

## Status

This repository currently contains the accepted implementation plan, decision
records, research evidence, and the validation scaffold. It does **not** yet
contain a runnable recognizer.

The first implementation milestone is:

```text
OBS WebSocket PNG or Gamescope PipeWire
  -> canonical RGB 1920x1080 frame
  -> fail-closed recognizer
  -> versioned Unix-socket NDJSON events
```

The runtime will be Rust. Python will only be used by an isolated, offline
adoption-time importer for upstream gzip/pickle resources.

## Project boundaries

- Keep upstream source code and history outside this repository.
- Adopt upstream release tags through a pinned, replay-gated import workflow.
- Treat OBS and Gamescope pixels as separate calibrated capture profiles.
- Never commit game screenshots, player data, imported resources, OCR models,
  or credentials.
- Keep the project private until upstream redistribution and licensing are
  explicitly resolved.

See [the Japanese implementation plan](docs/plan.ja.md),
[architecture overview](docs/architecture.md), and
[research evidence](docs/research.md).

## Development

Install [mise](https://mise.jdx.dev/), then use the repository entry points:

```text
mise trust
mise install
mise run check
mise run fix
mise run test
```

`check` is read-only, `fix` applies supported formatting fixes, and `test`
contains every reproducible repository check. Live Bazzite, OBS, Gamescope, and
GPU verification will remain separate explicit tasks.

## Licensing

No public license or redistribution grant is asserted at this stage. The
repository is intended for personal, private development while upstream and
game-asset licensing remain unresolved. Third-party runtime and model license
notices will be recorded before their artifacts are adopted.
