# scorepeek committed checkpoint

This file describes the state included in the commit that contains it. It is a
replace-in-place checkpoint, not a session log. Uncommitted working-tree state
is outside this checkpoint.

## Current milestone

- Milestone: **M1.2 — live catalog acquisition and sync orchestration**
- State: **in_progress**
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
- A dependency-free, credential-free dqn/iidxapi live JSON adapter boundary
  that accepts only content-SHA-256-pinned bytes, preserves nullable pack
  evidence, and rejects truncation, schema drift, revision mismatch, and
  duplicate rows before federation.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- The tests use synthetic, independently created fixture data only.
- The dqn live adapter parsed the 2026-08-15 endpoint response as 1,879 records
  at content SHA-256
  `b92bbba31b8f9c3f968afe8481f65aec411f95d4f211c19f671c67752d8d275d`.
  Those external bytes were used only for local verification and were not
  added to the repository.

## Unverified and target-only boundaries

- No Tachi or Textage live adapter exists yet. The dqn adapter consumes pinned
  bytes but no HTTP acquisition transport, cache, or scheduled/manual
  synchronization command exists yet.
- No real external-source snapshot has been federated, persisted, or activated.
  Catalog-update recognition replay, private capture corpus, OCR model, capture
  backend, field recognizer, event daemon, and the integrated live flow also
  remain unvalidated.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.

## Blockers and required approvals

- Connecting the first live adapter to acquisition requires selection and user
  approval of an HTTP runtime dependency, unless a dependency-free transport
  design with the same correctness and maintenance properties is established.
- Any new runtime, parser, capture, or training dependency requires user
  approval after version, license, alternatives, and host/bundle impact are
  presented.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Continue **M1.2 — live catalog acquisition and sync orchestration** by selecting
an HTTP runtime dependency and, after approval, implementing a bounded serial
dqn acquisition/cache vertical slice plus `scorepeek catalog sync`. Acquire the
existing per-host writer lock before network access, verify the pinned response
through `DqnLiveAdapter`, federate against the active catalog, and activate only
a healthy candidate. Add transport-status, content-length/size-limit, timeout,
and cache-write failure coverage before adding scheduled sync or the Tachi and
Textage paths. Do not mark M1 complete until all three source paths and manual
and scheduled sync satisfy the catalog activation gates. Catalog-update
recognition replay remains part of M8 because it depends on the later
recognition pipeline.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, repository bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | in progress |
| M1.1 | Catalog contract and local federation core | complete |
| M1.2 | Live acquisition and sync orchestration | in progress |
| M2 | Private corpus, layout measurement, synthetic renderer, and replay tooling | pending |
| M3 | OCR training/export and Python-to-Rust parity | pending |
| M4 | Portal reference capture and canonical-frame validation | pending |
| M5 | Gamescope/OBS candidate evaluation and backend selection | pending |
| M6 | Fail-closed field recognition and cross-field validation | pending |
| M7 | Deterministic session, versioned events, and NDJSON daemon | pending |
| M8 | Integrated catalog, holdout, and Bazzite release gates | pending |
