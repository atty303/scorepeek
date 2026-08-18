# scorepeek committed checkpoint

This file describes the state included in the commit that contains it. It is a
replace-in-place checkpoint, not a session log. Uncommitted working-tree state
is outside this checkpoint.

## Current milestone

- Milestone: **M4 bootstrap — offline canonical-frame and recognition spike**
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
  activation, and fsync boundaries.
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
  fsync boundaries, a 64 GiB per-source limit, a
  1,024-object limit, a 1 TiB aggregate limit, and separate 1,024-file/64 MiB
  fixture-manifest bounds.
- Immutable corpus-generation sealing that records every current fixture and
  canonical source-manifest digest under the ingest writer lock, publishes the
  generation by canonical SHA-256 with fsync boundaries, and never
  rewrites an older generation when later sources are ingested. The generation
  store is bounded to 128 files, 256 KiB each, and 32 MiB total.
- Local filesystem permissions, ownership, ACLs, and retention are operator
  responsibilities. Existing Unix modes and group/world writability are not
  acceptance criteria; scorepeek retains symlink, path-type, complete-byte,
  no-clobber, ownership-marker, and fsync integrity checks. Restrictive creation
  modes are best-effort defaults, not output guarantees.
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
  bytes and context is idempotent. Copy import hash-verifies a private staging
  snapshot; `--external` instead stores a private path locator outside dataset
  identity and rehashes the referenced bytes on every use. Moving a file can be
  repaired by reimporting identical bytes without changing source, recording,
  or generation identity. A completed recording bundle is reused after
  full-byte and typed-binding verification without another decode or packet
  probe.
- The first versioned domain normalizer for the exact FFV1/yuv420p/limited-range
  BT.709 1920x1080 observed contract. Its registry entry admits only capture
  profile `d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704`,
  pinned FFmpeg digest
  `9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee`,
  time base 1/1000, and explicit BT.709 range/space/transfer/primaries. Unknown
  or merely similar contracts fail closed.
  Canonical extraction publishes the normalizer, manifest, and selected PPM
  frames as one digest-bound private artifact.
- A scorepeek-owned canonical result layout measured only after normalization,
  with result header, title, artist, difficulty, level, notes, and current-score
  ROIs. The initial pure
  Rust screen spike classifies a result only when both fixed warm- and red-pixel
  predicates pass; otherwise it returns `unknown`. The recognition constructor
  requires valid normalizer/extraction evidence and matching file/pixel hashes,
  so bare PPM and observed extraction cannot enter recognition directly.
- A result-crop export boundary that first validates the canonical extraction
  and result predicate, then publishes six shared-layout PPM crops and a
  digest-bound private manifest. The offline OCR consumer requires the expected
  crop-manifest digest and registered normalizer digest, and cannot accept a
  bare observed or canonical PPM directly.
- A mise-pinned Python 3.13.7 and uv 0.11.7 offline environment with a committed
  `uv.lock` for PaddleOCR 3.7.0 and PaddlePaddle CPU 3.3.1. Python and its
  approximately 1.2 GiB development environment do not enter the Rust
  game-session dependency graph.
- An immutable `PP-OCRv6_small_rec` registration containing its official source
  URL, Apache-2.0 license reference, package compatibility, and exact archive
  and extracted-file sizes and SHA-256 values. Explicit acquisition publishes
  the 21 MiB model below a local content-addressed store, verifies every
  reuse, rejects unexpected archive entries, and inference receives only that
  verified local directory instead of auto-downloading.
- An independent official `PP-OCRv6_small_rec` ONNX registration pinned to
  PaddlePaddle revision `3d2d345e6a299891174f1397a72cdd81331359c7`, exact
  21,159,378-byte graph SHA-256, Apache-2.0 reference, and the same inference
  JSON/YAML hashes as the registered Paddle model. Explicit offline acquisition
  verifies and publishes that file below the same content-addressed model store;
  the Rust command never downloads a model.
- A diagnostic Paddle/Rust title path. The uv-locked Python producer accepts
  only a digest-bound canonical crop artifact, the registered Paddle model, and
  a canonical private candidate list. It retains the verified crop bytes and
  runs Paddle from a temporary snapshot of the verified registered model bytes,
  then writes the exact preprocessed float32 tensor, Paddle graph output, token
  orders, and CTC-constrained candidate scores to a digest-bound private
  reference. Rust uses ONNX Runtime 1.28 through `ort` 2.0.0-rc.13, verifies the
  registered graph, dictionary, reference, and bound crop bytes, independently
  reproduces the complete 3x48x320 BGR/resize/normalize input, and fails if the
  input/output tensors, token order, or parity candidate scores/ranking exceed
  their bounds. It then tokenizes every exactly encodable non-search title in
  the identified active catalog without normalization, scores their shared CTC
  trie, and returns a song only when explicit diagnostic absolute and runner-up
  bounds pass. Ties, unencodable catalogs, and insufficient evidence remain
  `unknown`; the command produces no free OCR text or accepted title.
- A Rust diagnostic title-candidate bridge with versioned comparison key
  `scorepeek-title-nfc-without-ascii-space-v1`. It applies NFC and removes only
  U+0020, preserves case, punctuation, other whitespace, and all other
  characters, excludes search-term aliases, and returns a song ID only when
  the fixed 0.95 diagnostic confidence bound passes and exactly one catalog
  song owns the matching key. Low confidence, no match, and cross-song
  collisions remain explicitly unknown. This open-text bridge is private
  evaluation only and does not produce an accepted title value.
- Immutable recording-dataset generations that bind every imported recording
  to five typed byte roles and revalidate their canonical manifest
  relationships as well as size and complete SHA-256. The generation digest is
  the reusable identity; caller dataset IDs are descriptive only.
- Explicit S3-compatible dataset push, pull, local verify, and remote verify
  commands in the offline corpus crate. Remote objects and generations use
  immutable content-addressed keys, objects precede generation publication,
  existing bytes are fully downloaded and hashed before reuse, and uploads go
  to unique scorepeek-owned staging keys before full remote verification and
  conditional server-side publication. Staging is deleted on success and every
  failure; a changed external source cannot leave bytes at the final key. No
  mutable latest pointer or delete command exists. Remote configuration excludes
  credentials and accepts production endpoints only as path-free HTTPS origins
  without userinfo, query, or fragment.
- Dataset verification parses all five roles as canonical typed schemas and
  revalidates source, recording, profile, observed media, and probe references.
  Local source/document/generation collections have count and aggregate-byte
  limits; pull preflights all missing capacity under the writer lock and
  rechecks it at publication. Dataset verify and push reject intermediate
  symlinks rather than reading outside the private store.
- Role-specific document size limits are enforced before remote GET. Downloads
  use unlinked temporary files, while crash-left scorepeek-owned
  source/document publication staging is recovered and fsynced under the
  writer lock before capacity accounting.
- `object_store` 0.14.1 with only its AWS feature, Tokio 1.53.1, and a direct
  use of the already-transitive `futures-util` 0.3.34 streaming interface and
  `url` 2.5.8 parser are approved offline-corpus dependencies. They do not enter the game-session
  runtime. Mise-pinned `rclone` 1.74.2 is a test-only S3-compatible server and
  does not enter a Rust binary.

## Verified in this checkpoint

- `mise run check`, `cargo clippy --locked --workspace --all-targets -- -D
  warnings`, and `cargo test --locked --workspace`: passed on the development
  host. The current workspace run covered 75 `scorepeek` library tests, 12
  binary tests, 44 offline corpus tests, and 8 offline Python OCR tests.
- `mise run catalog:schedule:systemd:verify`: passed without installing the
  release binary or user units.
- `cargo test --locked -p scorepeek-corpus`: passed all 44 offline corpus
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
  A separate external-source regression sealed without `source.media`, failed
  after its recording moved, rebound the locator by reimporting identical
  bytes, and then verified the unchanged generation. Invalid media leaves no
  locator, and a verified open source handle is not redirected by a path rename
  while in-place mutation is detected by its post-consumption hash.
  An in-memory object-store test exercised streaming upload, full-byte remote
  reuse verification, staging cleanup, bounded download, and same-size corrupt
  object rejection.
  Regressions also reject a source-path replacement between hash and stream
  inspection before publishing a recording binding, a self-consistent
  typed-role substitution, an intermediate
  content-directory symlink, typed-document oversize, dataset-generation
  capacity excess, stale owned staging, and endpoint
  userinfo/path/query/fragment.
- `mise run corpus:dataset:test:e2e`: passed against mise-pinned `rclone serve
  s3` on an exact loopback HTTP endpoint. The CLI imported a synthetic
  self-contained recording larger than the 8 MiB multipart threshold, sealed
  and locally verified it, pushed all six objects, observed rclone's initiate,
  part-upload, and complete-multipart operations, reused all six objects on a
  second push, remotely verified every byte, pulled to an empty store, and
  reproduced byte-identical source media. It was rerun after adding the bounded
  create-only PUT fallback used when the mock rejects server-side multipart
  conditional copy.
- An isolated real OBS/vkcapture gate registered the private
  14,785,693,017-byte Matroska/FFV1 recording at source SHA-256
  `53d4745e22e078db9b343896d17c0a63781afada1a664323fc0b12bab563c697`
  through `--external`. The importer indexed 27,499 FFV1 packet PTS and bound
  capture profile `d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704`
  without creating `source.media`; the store occupied 1.2 MiB while logical
  capacity and generation size retained all 14.8 GB. Seal and local full-hash
  verification produced one-recording generation
  `a85711a1fc183a916c3b8ab505744c6cd969ae270efb53b31c774aff72d9c11e`.
- The same gate normalized selected frames with artifact
  `0441099011fdd09d372d6c9b5e18d6c4f2da2809a653e01f8ccb55756d8658cf`.
  A separately invoked explicit FFmpeg transform produced the same file SHA-256
  for the representative result frame. Visual inspection covered mode select,
  music select, gameplay, result, failure overlay, transition, title,
  metadata, and score ROIs. Four stable result samples passed both committed
  result predicates; mode select, music select, gameplay, and two transitions
  returned `unknown`. After the independent review fix, the representative
  result was extracted again by hashing one source handle before and after
  FFmpeg consumption and reproduced extraction SHA-256
  `d318ca61bc36c1674484003008981df94d70f3fc02c0be9be6a21032e58b66fa`
  and pixel SHA-256
  `8e924f525ca2e52c8f9bf602d945a4e067d01629b2499b0ee487733214afb8aa`.
  Recognition required that expected extraction digest and revalidated the
  complete typed normalizer, extraction, and frame digest chain. These temporary private
  artifacts were not added to the repository or pushed to S3.
- An isolated synthetic CLI gate rendered three samples at manifest SHA-256
  `6a9aece0138816c972476d366df62cb4512b4488a178aedea86911133f80a2d0`.
  The first generated label and RGB8 crop were inspected together after a
  temporary PNG conversion; the generated output was not added to the
  repository.
- The current real title crop reproduced diagnostic text `ABSOLUTEEVIL` at
  confidence `0.9798454642295837`. Against an isolated three-source catalog at
  digest `65a8e164f3cb28e20c09114eecc3eb7200d32ed7c7383c7c04432053b78411eb`,
  the versioned exact comparison key resolved one candidate song ID,
  `6ef33da9-090a-500c-844a-8bffd14de63f`, corresponding to the visually
  inspected `ABSOLUTE EVIL` title. This remains a single-recording diagnostic,
  not holdout, confidence calibration, or accepted recognition evidence.
- The same title crop produced Paddle parity reference manifest
  `799addeaa8f9ce877169003f03fe837cf2b1e4fede759fdddf46f182edfca699`.
  Regeneration from retained crop bytes and a verified temporary Paddle-model
  snapshot produced the same manifest digest.
  The official ONNX graph at SHA-256
  `5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634`
  reproduced all 748,400 post-softmax CTC probabilities with maximum absolute
  error `0.0000056624413`, all 40 argmax tokens, the collapsed 12-token order,
  and a three-candidate constrained ranking. The maximum candidate log-score
  difference was `0.000026598829943935698`; both runtimes ranked the real
  `ABSOLUTE EVIL` song ID first. The model and private reference/candidate/crop
  artifacts remained outside the repository. A locked release build linked the
  CPU ONNX Runtime statically and changed the development-host `scorepeek`
  binary from 7,217,016 to 35,456,552 bytes.
- The Rust preprocessor reproduced all 46,080 Paddle input values for the same
  real title crop with maximum absolute error `0.0`. Against a freshly synced
  active three-source catalog at digest
  `65a8e164f3cb28e20c09114eecc3eb7200d32ed7c7383c7c04432053b78411eb`,
  the final diagnostic returned `catalog_coverage_incomplete` even with log
  probability `-1000` and runner-up margin `0` thresholds because some
  non-search titles cannot be exactly encoded by the registered dictionary.
  No song was promoted from the coverage-incomplete catalog. Synthetic
  regressions independently exercise exact CTC scoring, ties, insufficient
  absolute/runner-up evidence, and the coverage-incomplete fail-closed branch.
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
  quarantine category. The temporary XDG roots, external bytes,
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
  snapshot. Independent review reproduced cross-revision growth in Textage
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

- The first real normalizer, canonical-frame production, shared result ROIs,
  result-screen predicate, and PP-OCRv6 field recognition are only an offline
  single-recording spike. The
  current thresholds have no labelled holdout, profile-disjoint evidence, or
  support claim. The refined six-field crop run produced visually correct
  artist, difficulty, level, notes, and current-score text; the title omitted
  one internal whitespace despite high confidence. The diagnostic exact-key
  bridge restores that whitespace through one unique catalog candidate. The
  diagnostic parity gate now covers the Rust-produced input, official graph's
  post-softmax CTC tensor, token order, the reference's three-candidate ranking,
  and exact active-catalog trie scoring. Calibrated absolute/runner-up bounds,
  complete active-catalog dictionary coverage, temporal agreement, independent
  screen context, and accepted title semantics remain unimplemented.
  Music-select layout, scorepeek-owned model export, replay execution,
  catalog-update recognition replay, event daemon, and the integrated live
  flow remain unvalidated. The observed 649 ms CPU process and inference time
  is a single warmed development-host measurement, not a performance gate.
- The live `ObservedFrame`/domain-normalizer/`CanonicalFrame` runtime boundary,
  model-bundle promotion, and last-known-good model rollback remain
  unimplemented. Recognition accepts only digest-bound offline canonical
  extraction artifacts; it has no direct observed-frame input.
- The persistent systemd installer's custom unit-path linking, timer enablement,
  and unified disable path were reviewed but not deployed to the user's actual
  configuration. Only the non-persistent transient user-manager path was run.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.
- One real OBS/vkcapture game recording has passed isolated import, reimport,
  seal, local verification, canonical extraction, visual ROI inspection, and
  result-screen classification plus six-field PP-OCRv6 inference. The original
  file is the external dataset root; its copyless manifests and locator are in
  the durable operator-owned private corpus, while canonical/crop/OCR outputs
  remain reproducible derivatives and were not pushed to S3. S3-compatible push, multipart, reuse, remote
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
- Offline Python 3.13.7, uv 0.11.7, PaddleOCR 3.7.0, PaddlePaddle CPU 3.3.1,
  and the pinned PP-OCRv6 small multilingual recognition model were approved
  and added only to offline development; they do not enter the Rust
  game-session dependency graph.
- `ort` 2.0.0-rc.13 (MIT OR Apache-2.0) with a CPU ONNX Runtime 1.28 static
  binary was approved for the Rust parity spike. Default features, native TLS,
  tracing, copy-dylibs, and execution-provider features remain disabled; the
  selected `std`, `download-binaries`, `tls-rustls`, and `api-27` path added 16
  locked packages. The measured release-binary increase is recorded above.
- External-source access and reuse must remain within `docs/sources.md`; a
  source requiring new permission cannot be enabled until that permission is
  obtained.

## Next executable task

Implement the smallest value-free, catalog-digest-bound registered-dictionary
coverage audit, split by non-search variant kind, and use it to define the next
scorepeek-owned dictionary/model export boundary without silently dropping an
unencodable variant. After complete coverage is available, add digest-bound
replay execution over human-labelled independent sessions/titles before fixing
absolute or runner-up thresholds. Do not tune thresholds from the current
single recording, promote diagnostic commands into accepted recognition,
recognize bare PPM or `ObservedFrame`, auto-download a runtime model, or treat
the OBS profile, current ROIs, confidence, timing, or diagnostic thresholds as
supported.

S3 replication and another profile are intentionally deferred. When capture
work resumes, start **M3** with narrow Portal and Gamescope direct observed
profiles and calibrate each normalizer to the existing canonical layout without
moving route-independent ROIs.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, repository bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | complete |
| M1.1 | Catalog contract and local federation core | complete |
| M1.2 | Live acquisition and sync orchestration | complete |
| M2 | Observed-profile private corpus, synthetic renderer, and replay tooling | complete |
| M3 | Portal/Gamescope observed-frame profiles and calibration corpus | pending |
| M4 | Shared canonical layout, domain normalization, OCR training/export, and parity | in progress |
| M5 | Supported capture-profile evaluation and default selection | pending |
| M6 | Fail-closed field recognition and cross-field validation | pending |
| M7 | Deterministic session, versioned events, and NDJSON daemon | pending |
| M8 | Integrated catalog, holdout, and Bazzite release gates | pending |
