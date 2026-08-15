# scorepeek committed checkpoint

This file describes the state included in the commit that contains it. It is a
replace-in-place checkpoint, not a session log. Uncommitted working-tree state
is outside this checkpoint.

## Current milestone

- Milestone: **M1.1 — catalog contract and local federation core**
- State: **complete**
- Parent milestone: **M1 — catalog federation and activation** (`in_progress`)

## Included deliverables

- Versioned, strict synthetic fixture contracts for Tachi, Textage, and
  dqn/iidxapi observations, with immutable revision and content evidence.
- Deterministic, fail-closed federation anchored by UUIDv5 Tachi identities;
  exact-match cross-lineage corroboration; historical evidence retention; and
  quarantine for ambiguity, identity bridges, critical conflicts, regressions,
  and provisional records.
- Typed title variants, source chart assertions, product/version metadata,
  source bindings and attributes, dqn pack evidence, and explicit INFINITAS
  status.
- Private, content-addressed SQLite snapshots with semantic validation,
  single-writer locking, base-digest conflict detection, atomic manifest
  activation, fsync boundaries, and restrictive permissions.
- Synthetic regression coverage for adapters, federation, provenance,
  last-known-good behavior, deterministic snapshot round-trips, semantic
  tampering, and activation crash points.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- The tests use synthetic, independently created fixture data only.

## Unverified and target-only boundaries

- No live Tachi, Textage, or dqn/iidxapi acquisition adapter or scheduled/manual
  synchronization command exists yet.
- No real external-source snapshot, catalog-update recognition replay, private
  capture corpus, OCR model, capture backend, field recognizer, event daemon, or
  integrated live flow has been validated.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.

## Blockers and required approvals

- No blocker prevents starting M1.2.
- Any new runtime, parser, capture, or training dependency requires user
  approval after version, license, alternatives, and host/bundle impact are
  presented.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Start **M1.2 — live catalog acquisition and sync orchestration** by defining a
secret-free acquisition boundary and implementing the first strict live-source
adapter that converts pinned bytes into the existing `SourceSnapshot` contract.
Add fixture-backed failure tests for transport truncation, schema drift, and
revision mismatch before connecting it to the single-writer sync/activation
path. Do not mark M1 complete until all three source paths and manual and
scheduled sync satisfy the catalog activation gates. Catalog-update recognition
replay remains part of M8 because it depends on the later recognition pipeline.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, repository bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | in progress |
| M1.1 | Catalog contract and local federation core | complete |
| M1.2 | Live acquisition and sync orchestration | next |
| M2 | Private corpus, layout measurement, synthetic renderer, and replay tooling | pending |
| M3 | OCR training/export and Python-to-Rust parity | pending |
| M4 | Portal reference capture and canonical-frame validation | pending |
| M5 | Gamescope/OBS candidate evaluation and backend selection | pending |
| M6 | Fail-closed field recognition and cross-field validation | pending |
| M7 | Deterministic session, versioned events, and NDJSON daemon | pending |
| M8 | Integrated catalog, holdout, and Bazzite release gates | pending |
