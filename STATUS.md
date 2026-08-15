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
- A bounded serial dqn acquisition and private content-addressed cache using
  HTTPS-only `ureq`/rustls, a 30-second whole-request timeout, a reject-all
  redirect policy, 1 MiB declared/actual body limit, a 64-revision/64 MiB raw
  cache cap, and an honest scorepeek user agent.
- `scorepeek catalog sync`, which acquires the existing writer lock before
  network access, validates and caches exact bytes, federates against the
  active catalog, blocks snapshot-wide regressions, conditionally activates a
  durable snapshot under 32-generation, 64 MiB-per-file, and 512 MiB-total
  caps, and emits only aggregate quarantine counts.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- The tests use synthetic, independently created fixture data only.
- An isolated live `scorepeek catalog sync` fetched, validated, privately
  cached, persisted, and activated the 2026-08-16 dqn endpoint response as
  1,879 records at content SHA-256
  `b92bbba31b8f9c3f968afe8481f65aec411f95d4f211c19f671c67752d8d275d`.
  With no Tachi anchor in the isolated store, all 1,879 observations remained
  provisional and no song or availability binding was accepted. The command
  output contained only that aggregate count. The temporary private XDG roots,
  external bytes, and generated snapshot were removed after verification and
  were not added to the repository.

## Unverified and target-only boundaries

- No Tachi or Textage live adapter exists yet, and no scheduled synchronization
  exists. The dqn-only live snapshot cannot accept song or availability
  bindings without an active Tachi-anchored catalog.
- No live multi-source catalog has been federated or activated. Catalog-update
  recognition replay, private capture corpus, OCR model, capture backend, field
  recognizer, event daemon, and the integrated live flow also remain
  unvalidated.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.

## Blockers and required approvals

- `ureq` 3.4.0 with rustls was approved for the bounded live HTTP transport;
  no additional transport dependency is currently required.
- Any new runtime, parser, capture, or training dependency requires user
  approval after version, license, alternatives, and host/bundle impact are
  presented.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Continue **M1.2 — live catalog acquisition and sync orchestration** with the
Tachi live path so the active catalog has independently pinned identity/chart
anchors before adding Textage corroboration. Inspect and version the current
Tachi seed schema, keep Git inputs fixed to an exact commit, parse downloaded
data without executing it, and request approval before adding any required
parser dependency. Reuse the dqn writer-lock, bounded acquisition, private
cache, source-health, federation, activation, and aggregate-reporting
orchestration rather than creating a second sync path. Add scheduled sync only
after all three manual source paths satisfy their activation gates. Do not mark
M1 complete until manual and scheduled sync both pass. Catalog-update
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
