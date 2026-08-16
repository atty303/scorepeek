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
  exact-match cross-lineage corroboration; revision provenance with
  assertion-level normalization of unchanged evidence; and quarantine for
  ambiguity, identity bridges, critical conflicts, regressions, and provisional
  records.
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
- A strict Tachi live adapter for the exact-commit `songs-iidx`, SP-chart, and
  DP-chart JSON collections. It preserves typed main, alternate, and
  e-amusement CSV titles; imports only primary standard SP/DP charts; excludes
  search terms and known custom chart modes; derives positive INFINITAS evidence
  only from primary chart versions; and rejects schema drift, duplicate IDs,
  orphan charts, inconsistent levels, and duplicate primary chart keys.
- Bounded serial Tachi acquisition that resolves GitHub `main` to a commit,
  fetches raw files only at that commit without executing code, applies a
  30-second whole-request timeout and reject-all redirect policy, and keeps at
  most 8 private verified bundles or 512 MiB.
- `scorepeek catalog sync`, which acquires the existing writer lock before all
  Tachi and dqn network access, validates and caches exact bytes, federates both
  sources against the active catalog, blocks snapshot-wide regressions,
  conditionally activates a durable snapshot under 32-generation,
  128 MiB-per-file, and 512 MiB-total caps, and emits only source evidence and
  aggregate quarantine counts.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- The tests use synthetic, independently created fixture data only.
- An isolated live `scorepeek catalog sync` resolved Tachi commit
  `4ef9ca588424e1a98dc73421a49dd8efe3b37ddd`, validated and privately cached its
  three IIDX collections as 17,967 accepted song/chart rows at framed bundle
  SHA-256 `7f64941f017bf09d81f2c6e01a1aae7f23d42678957cfb812788986f8cb87c96`,
  and fetched the 1,879-row dqn response at content SHA-256
  `b92bbba31b8f9c3f968afe8481f65aec411f95d4f211c19f671c67752d8d275d`.
  The combined sync activated 2,548 Tachi-anchored songs at catalog digest
  `7b31c9e7fa72b39a905554ace30b8c46d37e24639b7a31861cf65c748f3da0fa`;
  51 dqn rows remained provisional and the rest resolved without another
  quarantine category. The 74,330,112-byte SQLite snapshot and all raw files
  had private permissions. The temporary private XDG roots, external bytes,
  and generated snapshot were removed after verification and were not added to
  the repository.
- An independent review reproduced a second-revision capacity failure in the
  initial implementation. After unchanged title, chart, and binding assertions
  were normalized across source revisions, the same live Tachi and dqn bytes
  were federated under a distinct 40-hex Tachi revision and published again.
  Both the first and second snapshots were 74,330,112 bytes, while the latest
  source-level revision remained recorded. Synthetic regressions also cover a
  sparse Tachi change and excluded custom/non-primary orphan charts. The
  review-only external bytes and generated snapshots were removed afterward.

## Unverified and target-only boundaries

- No Textage live adapter or scheduled synchronization exists. No live
  three-source catalog has been federated or activated. Catalog-update
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
Textage live path as independent title/artist/BPM/chart/INFINITAS
corroboration. Fetch the three declared JavaScript inputs locally, decode their
declared encoding, and accept only the documented assignment/literal grammar
without executing JavaScript. Request approval before adding any parser or
encoding dependency. Reuse the single Tachi+dqn writer-lock, bounded
acquisition, private cache, source-health, federation, activation, and
aggregate-reporting orchestration. Add scheduled sync only after all three
manual source paths satisfy their activation gates. Do not mark M1 complete
until manual and scheduled sync both pass. Catalog-update recognition replay
remains part of M8 because it depends on the later recognition pipeline.

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
