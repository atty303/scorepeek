# scorepeek committed checkpoint

This file describes the state included in the commit that contains it. It is a
replace-in-place checkpoint, not a session log. Uncommitted working-tree state
is outside this checkpoint.

## Current milestone

- Milestone: **M2 — private corpus, layout measurement, synthetic renderer, and replay tooling**
- State: **in progress**

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
- A strict Textage live adapter that decodes the three mutable inputs as
  Windows-31J without replacement and parses only their bounded constant,
  assignment, object, array, string, integer, comment, and static `fontcolor`
  grammar without executing JavaScript. It admits `actbl` rows only when their
  title and chart-data rows exist, imports complete standard SP/DP chart slots,
  preserves exact display data after source-specific static formatting
  extraction, and keeps partial chart slots unknown.
- Bounded serial Textage acquisition using HTTPS-only `ureq`/rustls, a
  30-second whole-request timeout, a reject-all redirect policy, 1 MiB per-file
  limits, and a private three-file framed-digest cache capped at 64 revisions
  and 64 MiB.
- `scorepeek catalog sync`, which acquires the existing writer lock before all
  Tachi, Textage, and dqn network access, validates and caches exact bytes,
  federates all three sources against the active catalog, blocks snapshot-wide regressions,
  conditionally activates a durable snapshot under 32-generation,
  128 MiB-per-file, and 512 MiB-total caps, and emits only source evidence and
  aggregate quarantine counts.
- Optional daily scheduling that always invokes the same `scorepeek catalog
  sync` entry point. The standard recommended route is a systemd user oneshot
  service with a persistent daily timer and up to six hours of jitter. Users
  may instead keep synchronization manual, use another scheduler, or start a
  non-persistent transient systemd timer. No recurring path is enabled
  automatically.
- Reproducible systemd unit verification, explicit install/disable tasks, and
  an isolated live gate that starts a manual sync while a timer-triggered sync
  owns the existing catalog writer lock. The scheduling layer is independent
  of acquisition mode so a future approved GitHub-managed catalog can preserve
  the same command while users select self-build or provided acquisition.
- A separate offline-only `scorepeek-corpus` workspace crate and binary. The
  game-session `scorepeek` crate has no corpus or training dependency, and the
  future Python training/export pipeline remains outside the runtime graph.
- An accepted capture/recognition design that separates opaque-profile
  `ObservedFrame` inputs, versioned domain normalizers, a conceptual RGB8
  1920x1080 `CanonicalFrame`, and field-specific OCR preprocessing. Capture
  routes are independently gated peers; none is a pixel correctness reference,
  and the normalizer does not model pipeline layers separately.
- Versioned, strict private-corpus ingest/source/replay contracts. Every source
  binds an opaque capture profile, immutable domain-normalizer artifact digest,
  and layout profile without exposing capture-pipeline classifications.
  Committed examples contain only opaque IDs, hashes, and non-personal classes;
  complete labels and media remain external.
- Bounded content-addressed private source ingest with an explicit absolute
  store root, single-writer locking, scorepeek-owned staging recovery,
  canonical manifests, immutable fixture-ID binding, idempotent content reuse,
  private permissions, fsync boundaries, a 64 GiB per-source limit, a
  1,024-object limit, a 1 TiB aggregate limit, and separate 1,024-file/64 MiB
  fixture-manifest bounds.
- Immutable corpus-generation sealing that records every current fixture and
  canonical source-manifest digest under the ingest writer lock, publishes the
  generation by canonical SHA-256 with private/fsync boundaries, and never
  rewrites an older generation when later sources are ingested. The generation
  store is bounded to 128 files, 256 KiB each, and 32 MiB total.
- Replay-suite validation that requires complete coverage of one sealed
  generation, reads canonical source manifests and media from the private
  store, binds exact source-manifest/extractor-manifest/parameter/frame/label
  digests, preserves source PTS and strict decode order, and always rejects
  duplicate IDs or session/episode/play/title/identical-frame groups crossing
  train, validation, or holdout splits. Each suite explicitly selects an
  in-profile contract or adds capture-profile disjointness for cross-profile
  evaluation.
- A strict `scorepeek-private-complete-label-v1` contract with distinct result,
  music-select, and non-recognition shapes. Replay bounded-reads canonical
  private label documents, validates typed field states, frame/revision and
  screen-class bindings, and fails closed at 64 KiB per label, 250,000 labels,
  and 4 GiB total. Existing managed-component symlinks are rejected before an
  ingest or generation operation mutates the store.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- `mise run catalog:schedule:systemd:verify`: passed without installing the
  release binary or user units.
- `cargo test --locked -p scorepeek-corpus`: passed all seventeen synthetic
  corpus tests, including idempotent private ingest, fixture-ID conflict
  rejection, canonical source/complete-label binding, pre-mutation symlink
  rejection, label cross-field and unreferenced-object rejection, decode
  ordering, same-index/cross-index grouped split isolation, and distinct
  in-profile/profile-disjoint evaluation behavior.
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
- An isolated live three-source `scorepeek catalog sync` reused Tachi commit
  `4ef9ca588424e1a98dc73421a49dd8efe3b37ddd` and the 1,879-row dqn response,
  and decoded, validated, and privately cached the three Textage inputs as
  19,055 accepted song/chart rows at framed bundle SHA-256
  `3c1291f96946279512632ec69e5bf0f8d49ff0b7e301e43457bfe36bd5ad4f81`.
  The candidate activated at catalog digest
  `bc0395b58e6e1a7b6a395be7823d4ca8f15e20c1a1eb29468ecf6c4c9e89da16`;
  711 Textage/dqn records remained provisional and 85 Textage records had
  chart conflicts, with no fuzzy or ambiguous merge. A repeat sync reused one
  Textage cache generation and one byte-identical 85,233,664-byte catalog
  snapshot. All scorepeek cache, manifest, lock, and snapshot paths had private
  permissions. Independent review reproduced cross-revision growth in Textage
  title and binding evidence; semantic assertions now reuse their original
  evidence while the latest source revision remains recorded. Synthetic
  regressions cover both an unchanged revision and a sparse attribute change.
- An isolated transient systemd user timer invoked the release
  `scorepeek catalog sync` against temporary private XDG data and cache roots.
  After the scheduled run acquired the catalog writer lock, a concurrent
  manual invocation against the same roots was started. Both completed
  successfully with byte-identical aggregate JSON output, demonstrating that
  the schedule and manual paths serialize through the same lock. The transient
  units, external source bytes, and generated catalog were removed after the
  gate; no persistent timer was installed or enabled.

## Unverified and target-only boundaries

- Real media ingest/extraction, private label authoring, episode generation,
  synthetic rendering, replay execution, layout measurement, catalog-update recognition
  replay, OCR model, capture backend, field recognizer, event daemon, and the
  integrated live flow remain unvalidated.
- The `ObservedFrame`/domain-normalizer/`CanonicalFrame` runtime boundary, OCR
  preprocessor, model-bundle promotion, and last-known-good model rollback
  remain unimplemented and unvalidated. Corpus metadata and split-contract
  behavior are synthetically verified but have not been exercised with real
  capture data.
- The persistent systemd installer's custom unit-path linking, timer enablement,
  and unified disable path were reviewed but not deployed to the user's actual
  configuration. Only the non-persistent transient user-manager path was run.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.

## Blockers and required approvals

- `ureq` 3.4.0 with rustls was approved for the bounded live HTTP transport;
  no additional transport dependency is currently required.
- `encoding_rs` 0.8.35 was approved for replacement-free Windows-31J decoding;
  no JavaScript parser dependency is used.
- The initial M2 contract and ingest/replay validation use only existing
  workspace dependencies. No media or training dependency has been added.
- Any new runtime, parser, capture, or training dependency requires user
  approval after version, license, alternatives, and host/bundle impact are
  presented.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Continue **M2** with deterministic episode/index generation, private label
authoring, and the independently redistributable synthetic renderer while
preserving the verified private-store, canonical metadata, complete-label,
replay-ordering, and grouped split safety properties. Before adding media
extraction or a learned normalizer, present the proposed tool or dependency with
its pinned version, license, alternatives, and host impact for approval.
Catalog-update recognition replay remains part of M8 because it depends on the
later recognition pipeline.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, repository bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | complete |
| M1.1 | Catalog contract and local federation core | complete |
| M1.2 | Live acquisition and sync orchestration | complete |
| M2 | Opaque-profile private corpus, layout measurement, synthetic renderer, and replay tooling | in progress |
| M3 | Domain normalization, OCR preprocessing/training/export, and Python-to-Rust parity | pending |
| M4 | Portal/Gamescope observed-frame profiles and canonicalization gates | pending |
| M5 | Supported capture-profile evaluation and default selection | pending |
| M6 | Fail-closed field recognition and cross-field validation | pending |
| M7 | Deterministic session, versioned events, and NDJSON daemon | pending |
| M8 | Integrated catalog, holdout, and Bazzite release gates | pending |
