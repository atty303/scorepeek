# ADR 0049: Distribute the CLI with cargo-dist

- Status: Accepted
- Date: 2026-08-26
- Supersedes: ADR 0047's custom deployment unit, canonical bundle manifest, activation and
  side-by-side rollback protocol; ADR 0048's remaining private-bundle creation, transfer and
  activation wording

## Context

The first cross-machine plan made scorepeek responsible for a private deployment format that bound
the executable, resources, build identity and host prerequisites in one canonical manifest. That
format duplicated integrity and compatibility responsibilities already owned by standard release
archives, the registered resource loaders and `scorepeek doctor`. It also made a normal Rust CLI
installation depend on scorepeek-specific activation and rollback machinery before any target use.

The executable and the operator's private catalog and model data have different distribution
boundaries. The executable can use an ordinary local Rust CLI release artifact. Catalog and model
bytes remain operator data with their existing manifests and loaders and cannot be included in a
release artifact.

## Decision

Cargo-dist 0.32.0 builds the local release artifact for `scorepeek` only. The sole target is
`x86_64-unknown-linux-gnu`. Cargo-dist's standard archive layout, release profile, glibc detection
and SHA-256 checksum are used without a scorepeek-specific outer manifest, per-file checksum,
build-identity manifest or host-prerequisite manifest. `scorepeek-corpus`, source tarballs,
installers, CI, hosting and public release automation are excluded.

The archive contains the `scorepeek` executable and cargo-dist's ordinary repository metadata. It
must not contain catalogs, OCR models, capture bindings, frames, player data or credentials.
Transfer integrity is checked with the archive's cargo-dist SHA-256 sidecar. Resource integrity is
checked by the existing catalog and model manifests when their loaders consume the resources.

An operator extracts the archive and copies `scorepeek` to `~/.local/bin`. No installer, activation
record, deployment manifest or side-by-side rollback protocol is added. Private catalog and model
resources are transferred independently below `$XDG_DATA_HOME/scorepeek` (or the corresponding
default data directory), and their production, acquisition, update and deletion remain separate
workflows.

Host compatibility remains a runtime and documentation concern. `scorepeek doctor` reports the
host inventory, and the supported Bazzite, PipeWire and Gamescope conditions remain in the plan and
profile contracts. The archive carries version `0.1.0`; no git revision, custom build ID or host
identity is embedded in the CLI or archive.

This checkpoint generates artifacts locally only. It does not create a tag, GitHub Release,
installer, release workflow, public artifact or remote state change. A later clean-Bazzite transfer
and launch test is a separate explicit authority boundary.

## Consequences

- `mise run check` validates the cargo-dist plan without building a release archive.
- `mise run dist:build` creates the local archive and checksum, while `mise run dist:test` verifies
  the checksum, archive membership, `--version` and `doctor` in isolated home and XDG directories.
- The target runtime needs neither the repository checkout nor mise, Rust or Python, but still
  depends on the documented host libraries and an independently supplied catalog and model.
- Public redistribution remains blocked until licensing and release policy are decided separately.
