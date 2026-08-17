# scorepeek committed checkpoint

This file describes the state included in the commit that contains it. It is a
replace-in-place checkpoint, not a session log. Uncommitted working-tree state
is outside this checkpoint.

## Current milestone

- Milestone: **M2 — observed-profile private corpus, synthetic renderer, and replay tooling**
- State: **complete**

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
  `ObservedFrame` inputs, versioned domain normalizers, a specified RGB8
  1920x1080 logical game canvas with one shared canonical layout, and
  field-specific OCR preprocessing. Capture routes are independently gated
  peers; none is a pixel correctness reference or owns the game layout.
- Destructive v2 private-corpus ingest/source/replay contracts. Every observed
  source binds only an opaque capture profile. Replay binds its normalizer,
  canonical frame contract, and shared canonical layout separately without
  accepting the removed v1 tuple or compatibility paths.
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
- Private complete-label authoring that validates strict typed and cross-field
  semantics before publishing canonical SHA-256-addressed documents under the
  shared writer lock. Publication is idempotent, private, capacity-bounded,
  recovers only owned staging entries, fsyncs its durability boundary, and
  emits no labelled field values.
- Deterministic replay-index generation from strict frame plans. Each generated
  index revalidates its canonical source manifest, stored media, and complete
  labels; derives episode IDs from opaque episode-group SHA-256 values; rejects
  non-contiguous episode reuse; and publishes canonical private indexes under
  1,024-object, 32 MiB-per-file, and 4 GiB-total bounds.
- A dependency-free `scorepeek-procedural-5x7-v1` synthetic title renderer. Its
  seed-only contract accepts no caller text, font, image, catalog, or private
  corpus input and produces byte-deterministic RGB8 512x96 P6 PPM crops plus a
  canonical manifest. This establishes the renderer boundary, not
  production-representative glyph coverage or a redistribution grant.
- Shaka Project static FFmpeg/ffprobe release `n8.1.2-1` (FFmpeg 8.1.2), pinned
  by platform URL and SHA-256 in `mise.lock`. The Linux x86-64 binaries are
  fully statically linked, about 92 MiB combined, GPLv3, and isolated below the
  mise prefix; they are offline corpus tools and do not enter the Rust or
  game-session runtime dependency graph.
- Bounded private media probing and explicit frame extraction. Probe binds
  immutable source/source-manifest evidence to exact tool binary digests,
  the sole video stream's explicit index, codec/pixel/color metadata,
  dimensions, time base, and contiguous FFV1 packet-order indexes with integer
  PTS under the destructive v4 schema; zero/multiple-video inputs, non-FFV1
  codecs, and missing packet PTS fail closed without a decoded-probe fallback.
  Only self-contained Matroska is accepted and streamed through stdin with the
  demuxer forced and only FFmpeg's `pipe` protocol enabled, preventing a media
  input from opening network or secondary filesystem resources.
  Extraction admits at most 512 strictly ordered probe-bound frames and 4 GiB,
  emits RGB8 P6 PPM with no frame-rate resampling, validates exact
  dimensions/bytes and pixel/file hashes, and requires FFmpeg's actual selected
  decoded-frame count, order, and PTS to match the packet probe before publishing
  private evidence using parent locking, exact ownership markers, no-clobber
  publication, and fsync.
  Extracted RGB8 frames remain observed evidence at source dimensions and are
  never presented as normalized `CanonicalFrame` or layout evidence.
- A high-level complete-recording importer that derives recording, fixture,
  and session identity from the immutable source SHA-256; derives a capture
  profile from a versioned capture context plus the observed media contract;
  and publishes exact source, profile, probe, and recording bindings without
  selecting a baseline profile, normalizer, or layout. Reimporting identical
  bytes and context is idempotent. Initial import copies and hash-verifies one
  private staging snapshot, enumerates FFV1 packet PTS from that exact snapshot,
  and publishes the observation only after its stream contract matches the
  capture profile. A completed recording bundle is reused after
  full-byte and typed-binding verification, without another media decode,
  probe, or source copy.
- Immutable recording-dataset generations that bind every imported recording
  to five typed byte roles and revalidate their canonical manifest
  relationships as well as size and complete SHA-256. The generation digest is
  the reusable identity; caller dataset IDs are descriptive only.
- Explicit S3-compatible dataset push, pull, local verify, and remote verify
  commands in the offline corpus crate. Remote objects and generations use
  immutable content-addressed keys, objects precede generation publication,
  existing bytes are fully downloaded and hashed before reuse, and no mutable
  latest pointer or delete command exists. Remote configuration excludes
  credentials and accepts production endpoints only as path-free HTTPS origins
  without userinfo, query, or fragment.
- Dataset verification parses all five roles as canonical typed schemas and
  revalidates source, recording, profile, observed media, and probe references.
  Local source/document/generation collections have count and aggregate-byte
  limits; pull preflights all missing capacity under the writer lock and
  rechecks it at publication. Dataset verify and push reject intermediate
  symlinks rather than reading outside the private store.
- Role-specific document size limits are enforced before remote GET. Downloads
  use unlinked mode-0600 temporary files, while crash-left scorepeek-owned
  source/document publication staging is recovered and fsynced under the
  writer lock before capacity accounting.
- `object_store` 0.14.1 with only its AWS feature, Tokio 1.53.1, and a direct
  use of the already-transitive `futures-util` 0.3.34 streaming interface and
  `url` 2.5.8 parser are approved offline-corpus dependencies. They do not enter the game-session
  runtime. Mise-pinned `rclone` 1.74.2 is a test-only S3-compatible server and
  does not enter a Rust binary.

## Verified in this checkpoint

- `mise run test`: passed on the development host, including all Rust library
  and binary tests and repository checks.
- `mise run catalog:schedule:systemd:verify`: passed without installing the
  release binary or user units.
- `cargo test --locked -p scorepeek-corpus`: passed all 39 offline corpus
  tests, including idempotent private ingest, fixture-ID conflict
  rejection, canonical source/complete-label binding, pre-mutation symlink
  rejection, canonical/private/idempotent complete-label authoring with owned
  staging recovery, label cross-field and unreferenced-object rejection, decode
  ordering, deterministic/idempotent replay-index publication, non-contiguous
  episode rejection, generated-index replay validation, byte-identical
  seed-only synthetic rendering, destructive rejection of the removed v1
  profile tuple, separate canonical-frame replay binding, one-to-one
  normalizer-artifact/capture-profile binding,
  same-index/cross-index grouped split isolation, and distinct
  in-profile/profile-disjoint evaluation behavior. The
  media tests used the mise-pinned FFmpeg/ffprobe binaries to generate a
  synthetic 1920x1080 Matroska/FFV1 source, index its sole video stream by
  packet order, and extract two exact decode-index/PTS selections as observed
  RGB8 PPM while checking actual decoded PTS. Regressions cover non-FFV1 and
  multi-video rejection, selected decoded-PTS mismatch, non-Matroska secondary resource rejection,
  cross-fixture source/profile substitution, private no-clobber publication,
  and marker-gated crash recovery. A complete-recording test generated and
  imported synthetic Matroska, repeated the import idempotently, sealed and
  verified its five-role generation, and compared stored source bytes exactly.
  An in-memory object-store test exercised streaming upload, full-byte remote
  reuse verification, staging cleanup, bounded download, and same-size corrupt
  object rejection.
  Regressions also reject a source-path replacement between hash and stream
  inspection before publishing a recording binding, an insecure stored-source
  mode on reuse, a self-consistent typed-role substitution, an intermediate
  content-directory symlink, typed-document oversize, dataset-generation
  capacity excess, stale owned staging, and endpoint
  userinfo/path/query/fragment.
- `mise run corpus:dataset:test:e2e`: passed against mise-pinned `rclone serve
  s3` on an exact loopback HTTP endpoint. The CLI imported a synthetic
  self-contained recording larger than the 8 MiB multipart threshold, sealed
  and locally verified it, pushed all six objects, observed rclone's initiate,
  part-upload, and complete-multipart operations, reused all six objects on a
  second push, remotely verified every byte, pulled to an empty store, and
  reproduced byte-identical source media.
- An isolated real OBS/vkcapture recording gate imported the private
  14,785,693,017-byte Matroska/FFV1 recording at source SHA-256
  `53d4745e22e078db9b343896d17c0a63781afada1a664323fc0b12bab563c697`.
  The destructive v4 importer indexed 27,499 FFV1 packet PTS from the
  hash-verified staging source and completed in 23.49 seconds;
  an identical reimport reused the bound probe and source without FFprobe or
  another copy, returned the same summary, and completed in 12.81
  seconds using only full-byte and typed-bundle verification. Dataset seal and
  local verify completed in 6.39 and 6.40 seconds and produced the v4
  one-recording, five-object generation at
  `2b70e816fcdfb8bdda73e8a3ebcfb714220d22811516cd35331614745d575c5e`.
  Two real frames at decode indexes 0 and 2 were extracted only after actual
  decoded PTS matched the packet probe. SHA-256 source and capture-profile
  identities remained unchanged; the v4 probe and transitive generation
  identities changed intentionally.
  The isolated store was not pushed to S3 and is not a persistent corpus.
- An isolated synthetic CLI gate rendered three samples at manifest SHA-256
  `6a9aece0138816c972476d366df62cb4512b4488a178aedea86911133f80a2d0`.
  The first generated label and RGB8 crop were inspected together after a
  temporary PNG conversion; the generated output was not added to the
  repository.
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

- Real media extraction, canonical-frame production, shared-layout measurement,
  replay execution,
  production synthetic glyph/style coverage, catalog-update recognition
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
- One real OBS/vkcapture game recording has passed isolated import, reimport,
  seal, and local verification. It has not been retained in an operator-owned
  durable corpus or pushed to S3. S3-compatible push, multipart, reuse, remote
  verification, and pull have been verified against the local rclone server,
  not a real private bucket. Live credential, provider-specific addressing,
  TLS, bucket lifecycle, and provider behavior remain an explicit external
  gate.

## Blockers and required approvals

- `ureq` 3.4.0 with rustls was approved for the bounded live HTTP transport;
  no additional transport dependency is currently required.
- `encoding_rs` 0.8.35 was approved for replacement-free Windows-31J decoding;
  no JavaScript parser dependency is used.
- The approved `github:shaka-project/static-ffmpeg-binaries@n8.1.2-1` media
  tool is pinned through mise. It added no image, font, game-session runtime,
  or training dependency.
- `object_store` 0.14.1 (Apache-2.0) with its AWS feature and Tokio 1.53.1
  (MIT) were approved for offline S3-compatible corpus transport. Direct
  `futures-util` 0.3.34 (MIT OR Apache-2.0) use exposes its already-transitive
  streaming interface and `url` 2.5.8 (MIT OR Apache-2.0) parses strict endpoint
  origins; both add no dependency graph. Mise-pinned `rclone`
  1.74.2 (MIT) replaces the rejected Java-based test-server candidate and is
  used only by focused E2E.
- Any new runtime, parser, capture, or training dependency requires user
  approval after version, license, alternatives, and host/bundle impact are
  presented.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Choose an operator-owned durable private corpus root and private S3 remote,
import the verified OBS/vkcapture recording there, then seal, push, and remotely
verify its first persistent generation. After that, start **M3** with narrow
Portal and Gamescope direct `ObservedFrame` vertical spikes on the Bazzite
target. Record each real observed contract and collect private calibration
evidence without assigning a normalizer or layout at source ingest. Once the
peer profiles are observable, continue M4 by defining the shared logical-game
`CanonicalFrame`/layout artifact and calibrating one deterministic normalizer
per profile to it. Do not select a captured route as a pixel reference, create
route-local layouts, or measure canonical ROIs from raw extractions. Real media
and frames remain external and private.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, repository bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | complete |
| M1.1 | Catalog contract and local federation core | complete |
| M1.2 | Live acquisition and sync orchestration | complete |
| M2 | Observed-profile private corpus, synthetic renderer, and replay tooling | complete |
| M3 | Portal/Gamescope observed-frame profiles and calibration corpus | pending |
| M4 | Shared canonical layout, domain normalization, OCR training/export, and parity | pending |
| M5 | Supported capture-profile evaluation and default selection | pending |
| M6 | Fail-closed field recognition and cross-field validation | pending |
| M7 | Deterministic session, versioned events, and NDJSON daemon | pending |
| M8 | Integrated catalog, holdout, and Bazzite release gates | pending |
