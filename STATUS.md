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
- A music-select spike measured from the same canonical profile. Its independent
  cyan-header and green-level-column predicate classifies the two retained
  representative frames fail closed, then exports one selected-title crop and
  twenty fixed visible-list row slots as a digest-bound private artifact.
  Geometric list slots deliberately preserve separators, clipped rows, and other
  non-title content for downstream rejection instead of silently filtering them.
- A canonical private music-list row observation-draft v2 contract with mutually exclusive
  stationary, scrolling, selected, clipped, non-title, and unknown annotations and one annotation
  per geometric frame/slot. Temporal drafts record locked/dimmed availability and standard,
  INFINITAS-blue, or LEGGENDARIA-purple color independently from motion; an inserted unlock-condition
  bar is explicit non-title content. Inspection remains shape-only and unverified. The separate
  verifier rehashes the canonical extraction manifest, full canonical PPM, crop manifest, and crop
  PPM, confirms the fixed ROI against canonical pixels, and recomputes reported RGB L1 before
  returning verified evidence.
- A versioned complete-pair motion request and artifact contract. Every pair binds adjacent decode
  indexes from one extraction, requires explicit motion plus exactly twenty semantic row
  annotations for each frame, rehashes both canonical frames and all 21 crops per frame, and records
  twenty row RGB L1 values plus their checked aggregate. Measurement never infers a motion label
  from pixels, creates but does not replace its canonical output, and a separate verifier recomputes
  the complete artifact.
- A create-only complete-pair review plan that re-verifies the motion artifact and preserves every
  row occurrence, current annotation, and pair motion while grouping only exact pixel-SHA-256-
  identical crops. It does not derive labels from color, brightness, OCR output, or motion values.
- A create-only partial review application that reconstructs the exact-crop plan from the verified
  artifact, requires canonical human decisions bound to the plan SHA-256, rejects duplicate,
  unknown-group, and explicit-unknown decisions, and leaves every omitted group unchanged. Its
  output is a new motion request that can be measured and verified through the existing evidence
  path; it never promotes brightness, color, OCR, or motion measurements into semantic labels.
- A mise-pinned Python 3.12.13 and uv 0.11.7 offline environment with a committed
  `uv.lock` for PaddleOCR 3.7.0, PaddlePaddle CPU 3.3.1, and Apache-2.0
  `paddle2onnx` 2.1.0. Python and its
  approximately 1.2 GiB development environment do not enter the Rust
  game-session dependency graph.
- A registered `paddleocr-v3.7.0-training-source.json` fixes the official
  PaddleOCR source URL, commit, Apache-2.0 license, training/export entrypoints,
  PP-OCRv6 small recognition configuration, and requirements-file digests. It
  establishes a reproducible offline source boundary without vendoring upstream
  code. `ocr:training-source:verify` independently passed against the pinned
  checkout before the bounded private training and export runs recorded below.
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
- A value-free registered-dictionary coverage audit that verifies the exact immutable dictionary,
  binds aggregate output to the active catalog digest, splits every non-search variant by display
  kind, and reports unsupported-character and CTC-timestep rejections without exposing or silently
  dropping catalog strings.
- A create-only private scorepeek-owned title-model export-requirements artifact. It retains every
  Unicode scalar represented by the registered baseline dictionary, appends every scalar required
  by every active non-search catalog variant, raises the CTC timestep count to the complete set's
  longest exact alignment, and binds catalog and baseline digests, ordered scalar dictionary,
  class/timestep shape, and an explicit batch-timestep-class float32 logits contract. This defines
  the export boundary but does not train or export a model.
- A create-only private title-model preparation boundary that turns those complete-catalog
  requirements into a Paddle dictionary and exact title-disjoint label lists. It binds the pinned
  PaddleOCR source/config, rehashes an explicit one-to-one crop-path map, rejects labels outside the
  dictionary, preserves U+0020 through `use_space_char`, derives the recognition width from the
  required CTC timestep count, and records aggregate coverage without exposing title values in its
  summary. A separate export record binds selected Paddle and ONNX bytes to the preparation while
  remaining provisional, non-distributable, unaccepted for runtime, and explicit that actual model
  tensor shape is not yet verified.
- A Rust diagnostic title-candidate bridge with versioned comparison key
  `scorepeek-title-nfc-ucd17-exact-then-ascii-width-fold-v2`. Its first tier applies Unicode
  17 NFC and removes
  only U+0020. Only when that tier has no candidate, its fallback maps U+FF01 through U+FF5E to
  ASCII and removes U+0020 and U+3000. It does not apply general NFKC, alter case, fold
  halfwidth kana, or change other whitespace and characters. Exact-tier candidates take
  precedence over fallback collisions. The bridge excludes search-term aliases and returns a
  song ID only when the fixed 0.95 diagnostic confidence bound passes and exactly one catalog
  song owns the resolved tier's key. Low confidence, no match, and cross-song collisions remain
  explicitly unknown. This open-text bridge is private evaluation only and does not produce an
  accepted title value.
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

- `mise run check` and the complete `mise run test` entry point passed on the development host.
  The current workspace run covered 84 `scorepeek` library tests, 13 binary tests, 55 offline
  corpus tests, 24 offline Python OCR tests, and the recording-dataset E2E gate.
- The prior session's 3,061 provisional private labels and bound crops were recovered from its
  temporary tree into an operator-owned stable private-artifact root, with all crop paths and
  complete file/pixel SHA-256 evidence revalidated after relocation. The resulting song-disjoint
  manifest contains 2,362 training, 329 validation, and 370 evaluation labels across 1,119 songs;
  it remains `permission_not_recorded` and is not accepted holdout truth.
- A live catalog sync activated catalog
  `ceabe2931815c492b9eb088282ab6df55cabff2545fd9d8de3e0ae11b1b2b541`. Its complete-catalog
  requirements retain 4,810 non-search variants, require 18,725 output classes and 65 CTC
  timesteps, and report complete scalar coverage. The real private preparation rehashed every crop
  and produced a 520-pixel input width, complete scalar dictionary, and exact split lists. The
  requirements manifest is
  `dff306998233b7c4d70824e1326cb0eb1b3eced017695e1b094089f18563e1ca`; its non-blank token order
  places U+0020 last to match Paddle's `use_space_char` behavior. The
  preparation manifest is
  `e2544e8d11b7c4e6fdb4448b512dd31d8e72a2c1d06b2539f8e93a3175699c23`; its dictionary is
  `b3a5e331f8f5ccf70a228c7d831ef945b87aa7a098b70f68f2b58fea88c1358f` and derived config is
  `ab74e660bd11f3c42a90a8933db57c02f8b73add5fedb437be2e7cca27a51b1d`. Each split now
  carries a preparation-bound crop-digest sidecar; the pilot trains only from verified temporary
  snapshots and validation/replay use one digest-checked read per crop.
- The official PP-OCRv6 small recognition training checkpoint is registered at exact SHA-256
  `25c9bd54b0e5900916e8bb6ada938abeffb1eac1baedac0ca54a45b1c9310825` (124,912,348
  bytes) and stored content-addressed outside the repository. Its 422 tensor names match the
  prepared graph: 418 shapes match directly and the four class-dependent CTC/NRTR tensors differ
  only by the 15 appended characters. A create-only private initializer maps every baseline
  dictionary character plus space and all NRTR special tokens, leaving only those 15 new classes
  randomly initialized. Aggregate open-text exact recognition on all 329 provisional validation
  crops measured 275/329 (83.6%) before fine-tuning; both scratch and Paddle's ordinary
  shape-matched-only load measured 0/329 in the same probe. The initialized checkpoint is
  `6b3774ae7c7ef42df47220bd69b1b67f9db6afbe7650782919f53ee451cff72a` (124,965,680
  bytes). This selects the mapped initializer for the private pilot but does not make the
  provisional labels accepted holdout truth or establish a release threshold.
- The bounded CPU pilot stopped after the first optimizer step improved provisional validation.
  The mapped initializer measured 275/329 strict open-text exact and one step measured 277/329.
  The selected one-step checkpoint is
  `a0c2c70277a5b2d3c032366ec5ca0fc4d082b4f3fbb71271e29b0c62938806e2` (124,965,688
  bytes), bound by pilot manifest
  `2bf72df1015dd62bd10dae0e1ca25a1c4c9170c8f0365e4fc803a698b13f7a38`.
  Its private Paddle/ONNX export manifest is
  `ba7d4dc3ab90ae0da9291d4c76099b9d1e4f298261bb47659d1b856242735ea3`; the ONNX graph is
  `70faf846ab2625d3de0602cbed3a492a3bea631f3627483c8872873a706f4c5c` (21,155,598
  bytes). Rust reproduced the Paddle `[1,65,18725]` tensor with maximum absolute error
  `0.0000011324883`, the exact prepared dictionary order, and identical argmax and collapsed token
  order. Paddle training/export/conversion now run with explicit timeout and process-group cleanup;
  cleanup covers spawn-time signals, timeouts, non-zero leaders, and surviving descendants. Export
  subprocesses write outside the artifact root until verified publication. Preparation and model
  directory publication serialize the final no-clobber check and rename under one parent lock.
- Past-session evidence and retained recording bytes restored the ignored recognition spike to
  the current private-artifact root. A low-frequency scan found the earlier result transition at
  PTS 140000 and the stable result at PTS 145000; only the stable frame was admitted beside the
  existing PTS 190000 frame. Both visibly contain `ABSOLUTE EVIL`. The older PTS 190000 crop bound
  a superseded layout and was regenerated from its retained canonical extraction under the current
  layout. Replay manifest
  `411d34b487c9dc42d2fd10214c5149ec38c674072e8fdae7242e74e7045719a3` compares those two
  fully revalidated result-crop artifacts and all 370 provisional evaluation rows, and records each
  crop's extraction, canonical-frame, normalizer, layout, and title-byte provenance. The initializer and one-step
  pilot measured 293/370 and 295/370 strict open-text exact respectively, and both decoded the two results as
  `ABSOLUTEEVIL`. Under the versioned exact comparison key, the initializer measured 301/370 and
  2/2 results while the pilot measured 298/370 and 2/2 results. The open-text improvement conflicts
  with a three-row regression in scorepeek's catalog comparison metric; the one-step checkpoint is not selected over the initializer and
  no threshold was fixed.
- Music-select presentation was measured before changing either training or live recognition. On
  all 3,061 known standard crops, a channel-maximum RGB transform changed the official model's
  strict exact count from 2,715 to 2,725 and comparison-key exact from 2,859 to 2,871; the mapped
  initializer changed from 2,515 to 2,524 and 2,607 to 2,613 respectively. The same untrained
  initializer was neutral on the 24 INFINITAS-blue catalog resolutions and lost four of 232
  LEGGENDARIA-purple unique resolutions, with no conflicting unique resolutions. A versioned
  `scorepeek-title-channel-max-rgb-v1` pilot therefore transformed only the existing standard
  training crops and applied the identical transform to validation and replay. One optimizer step
  improved validation from 275/329 to 276/329, but held-out provisional evaluation regressed from
  298/370 to 297/370 strict exact and from 303/370 to 300/370 comparison-key exact; both result
  observations remained 2/2. The candidate checkpoint is
  `2ee81f70714bb9de25c72bbdcb4715b4f765f7acaac2278988d04b1e1bb6bfdb`, its pilot manifest is
  `a0c45834395641abf83780757e114cd7e5e47b2f7d816f51f42b65a2d3e40466`, and its replay manifest is
  `93b365610b0e9076a7542f284948b0db7e826480797ac085c92c48cf55c2a83e`. The transform is rejected as
  a default. Offline pilot, replay, and export artifacts must now carry an explicit registered
  presentation-transform ID so a future accepted model cannot silently train and run under
  different presentation contracts. Export parity references and Rust contract summaries retain
  and validate the same ID rather than silently reverting to identity preprocessing.
- The new title-model requirements regressions preserve baseline scalar coverage, append missing
  catalog characters, increase exact repeated-token CTC alignment length, and reject invalid or
  empty variant sets. The music-list row regressions cover all six explicit states, require
  adjacent decode indexes and exact full-row RGB comparison counts for temporal states, keep
  locked/dimmed and the two non-standard color domains orthogonal to motion, preserve unlock bars
  as non-title content, reject
  duplicate annotations of one frame/slot, reject non-canonical documents, and bound reads across
  path replacement or file growth. Synthetic artifact regressions prove that verification detects
  crop tampering and recomputes L1 from canonical-frame-bound bytes.
- Targeted recognition tests passed with the added music-select predicate and
  21-crop artifact contract. The offline OCR contract suite passed 20 tests,
  including exact validation of the selected-title and twenty list-slot files and the Unicode 17
  exact-first width-fold contract. All eight targeted Rust title-association tests also passed.
- `mise run catalog:schedule:systemd:verify`: passed without installing the
  release binary or user units.
- `cargo test --locked -p scorepeek-corpus`: passed all 55 offline corpus
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
- The deliberately paced HYPER full-list OBS/vkcapture recording was imported copylessly at
  38,235,570,222 bytes and source SHA-256
  `f1b5cb9687ee96052be9517056eef58fdc3cd89d96c191b098030ddfc04f2294`.
  Its 42,325 FFV1 packets retain the same capture profile. Dataset generation
  `33f568d728caa927dc36be3519448e31e5b8a0d7deb9fd65910d8c22ede117af`
  binds both recordings, 10 objects, and 53,024,235,103 logical bytes and passed local full-hash
  verification. No recording copy or visual derivative was committed or pushed.
- A reproducible exploratory extraction selected 241 adjacent frame pairs from source PTS 120000
  through 660017 at a 135-decode-index stride. The extractor now compacts regular adjacent runs
  into one equivalent FFmpeg select expression; the previous one-term-per-index expression failed
  at this size while a four-frame same-path probe succeeded. All 482 frames normalized to the
  canonical contract at extraction SHA-256
  `334cf026266f7eab306ef3ad90c0db2a4ce4388538f040370ed0cafad05f7710`.
  The difficulty-independent saturated-level predicate classified all 482 as music-select-family
  frames; observed colored-level counts were 13,357 through 52,095 against the retained 1,000-pixel
  floor. One visually confirmed complete standard row passed the artifact-bound verifier at
  recomputed RGB L1 21,673. The temporary extraction and crops remain private derivatives.
- Exploratory full-list aggregation exposed a clean candidate pair-level motion gap: 183 pairs
  were at or below 2,279,433 RGB L1 across all twenty slots and 57 were at or above 10,737,346,
  after excluding the final filter-menu pair. Individual rows overlap (stationary maximum 287,170;
  scrolling minimum 107,403), so no row-only stability threshold is accepted from this clustering.
  Complete row presentation annotations are still required before accepting a profile- and
  presentation-bound pair-level threshold.
- The new complete-pair gate generated and independently reverified a private
  `scorepeek-private-music-list-motion-artifact-v1` for 240 adjacent pairs (the final filter-menu
  pair remains excluded). It rehashed 480 canonical frames and their complete crop artifacts and
  initially retained all 240 motion labels and both sets of row semantics as
  `unknown: pending-review` rather than deriving ground truth from the measured distribution.
  Human review of all fifteen private comparison pages found every pair at or below 2,279,433 to
  be stationary and every pair at or above 10,737,346 to be scrolling, with no pair in the gap.
  The regenerated private request and independently reverified artifact therefore contain 183
  stationary, 57 scrolling, and zero unknown motion labels. Verified aggregate RGB L1 spans 0
  through 40,782,712. At that motion-only checkpoint, before the row-review applications described
  below, all 9,600 row presentation annotations were `unknown: pending-review`; the private
  requests, artifacts, and comparison pages remain temporary derivatives outside the repository.
- The complete-pair review planner independently rehashed that private artifact and retained all
  9,600 row occurrences in 7,977 exact-pixel groups. There are 1,621 duplicate groups, saving 1,623
  repeated visual decisions without treating similar but non-identical pixels as equivalent. The
  4,815,725-byte canonical private plan remains outside the repository. Exploratory brightness and
  hue ordering successfully surfaced locked/dimmed and LEGGENDARIA-purple candidates, but also
  selected, clipped, unlock-condition, and scrolling UI samples; those measurements therefore
  remain review-ordering hints rather than labels.
- Review disposition now covers all 7,977 exact-pixel groups and all 9,600 occurrences. Full-frame
  inspection established that every one of the 420 slot-10 groups carries the selection boundary,
  so selection takes precedence over crop-only clipping or apparent emptiness. They cover 480
  occurrences. Review of all 448 exact groups observed directly below those selections retained
  exactly 36 groups and 42 occurrences as explicit unlock-condition bars; partially overlaid BIT
  text and ordinary titles were not promoted. Fixed list geometry separately established that
  stationary slots 0, 3, 9, and 19 are obscured by other UI, yielding 1,159 `clipped: obscured`
  groups and 1,464 occurrences. The earlier full-frame pass retains 18 `clipped: right` groups and
  26 occurrences.
- Private brightness and foreground-color features were used only after contact-sheet calibration
  exposed disjoint reviewed clusters and an empty ambiguity band. The settled complete available
  title set contains 3,062 standard groups (3,990 occurrences), 24 INFINITAS-blue groups (24
  occurrences), and 299 LEGGENDARIA-purple groups (382 occurrences). The private canonical decision
  document at SHA-256 `2793a9c28702918160bb42ff2a41d228ad7beabe497ff42888bb379407c555bc`
  therefore settles 5,018 groups and 6,408 occurrences. The plan- and decision-digest-bound private
  disposition at SHA-256 `6fb95524787bf04ba401398a1e181c11347a2aa85d52df5f2008f46f94b4eeeb`
  records every remaining group as reason-bearing unknown: 2,166 vertical-motion groups, 351
  locked rows whose color domain is not observable without correction, 438 possible right-clips,
  and four intentionally redacted secret titles. Those 2,959 groups cover 3,192 occurrences and
  were not guessed.
- Partial review application independently reconstructed and verified the original plan, accepted
  all 5,018 decisions, and applied them to exactly 6,408 occurrences. The resulting private request
  SHA-256 is `6360ddb432dd5394ada3a4eb34d7a55732ec859a4d934df0163822ae534f648c`.
  Regeneration and independent verification produced private motion artifact SHA-256
  `8bcd7851f96bb61928252b0ae7b21799f01098f777b2bd4220574bbd3750b163`
  with the existing 183 stationary and 57 scrolling pair labels, zero unknown pair-motion labels,
  and aggregate RGB L1 from 0 through 40,782,712. No review sheet, captured pixel, decision,
  disposition, request, or artifact was added to the repository.
- The catalog-bound provisional-label gate now exports only active-catalog songs with a confirmed
  INFINITAS presence and an SP HYPER chart, excluding search aliases while preserving every display
  variant's lineage, revision, content digest, and rights statement. Against catalog
  `65a8e164f3cb28e20c09114eecc3eb7200d32ed7c7383c7c04432053b78411eb`, the retained private
  candidate artifact contains 1,879 songs at SHA-256
  `e2273634f478026a59d9c7f82dacf518a41481070bfd926356e171f9d1903d6a`. Rehashed application of
  the exact-first v2 key to exactly the 3,062 settled stationary standard groups produced 2,611
  provisional crop labels and 451 reason-bearing unknowns: 363 below the fixed 0.95 diagnostic
  confidence, 84 without a catalog key, and four with ambiguous display text. Every label records crop
  file/pixel digests, catalog source provenance, and `permission_not_recorded`; no unknown, blue,
  purple, locked, selected, clipped, obscured, scrolling, unlock-condition, or redacted row was
  promoted. Two independent inference runs produced semantically identical label/unknown payloads
  at digest `9eb7a3a761d395aeb5880b9144be8f13ea20ab94df43768158fc2b1ff16898de`; only elapsed-time
  metadata differed. The first create-only
  private artifact is SHA-256
  `7a2b1a4368419c409626f194019dcd833818031a95ec57d7bf4197cbce72b496` and remains outside the
  repository. These are automated provisional training inputs, not accepted labels or holdout
  evidence.
- The prior visual audit was hash-rebound to all 451 reason-bearing OCR unknown groups that remain
  after the v2 automated pass. It accepted 450 groups and retained `G06461` as excluded because it
  is the purple LEGGENDARIA `VERSION` UI rather than a song title. The resulting private label
  artifact at SHA-256
  `53aaedaca3efee110d5378f8b7277fcefe00e8e9e255d40ad794f9ed6d4cee5a` contains 3,061
  exact-pixel groups, 3,986 occurrences, and 1,119 catalog song IDs: 2,611 automated associations
  plus 450 visually reviewed associations, with zero remaining unknowns or catalog gaps. Its
  451-decision audit is bound at SHA-256
  `8e8326d08aca6e87ca1fea2ec121e5ad47b481da470d61a22145d2342d97d6cf`; rerunning the private
  generator reproduced both hashes. Every retained label remains `permission_not_recorded` and
  provisional rather than accepted holdout truth. The fallback automatically resolves six groups,
  including both ASCII OCR observations of fullwidth `ＰＡＳＴＥＬＩＳＭ`. Three `Turii` groups
  are corrected from Tachi's `alternate_display` wave-dash value to its catalog-authoritative
  `in_game_display` fullwidth-tilde value. Catalog acquisition and federation remain unchanged.
- A private `scorepeek-private-title-training-input-manifest-v1` now accepts the user-supplied
  candidate, automated-label, visual-audit, final-label, crop, and source artifacts only at their
  exact SHA-256 bindings. It rejects path, JSON, schema, required-field, UUID, permission-status,
  and split-contract mistakes, then assigns every crop for one catalog song to the same deterministic
  train, validation, or evaluation split. It retains the music-list origin and
  `permission_not_recorded` status and explicitly records that these are provisional, not accepted
  holdout truth. The private artifact inputs are trusted operator inputs; this boundary detects
  accidental contract/binding mistakes but does not independently re-adjudicate every label against
  every source artifact.
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
- Two music-select frames at source PTS 110000 and 270000 were normalized once
  into retained ignored private artifacts at extraction digest
  `93d2a0d0338eb3941cc70663f944dcfbbc20f3ccf60d525118ace64113e09589`.
  Both passed the new predicate. PP-OCR read the selected titles as
  `ABSOLUTEEVIL` and `ANEMONE` above 0.999 confidence. Across visible list rows,
  nine observed strings independently resolved to unique songs in active
  catalog `65a8e164f3cb28e20c09114eecc3eb7200d32ed7c7383c7c04432053b78411eb`:
  those two titles plus `Apocalypse ~dirge of swans~`, `bass 2 bass`,
  `Chewingood!!!`, `COLOR BURST`, `Critical Crystal`, `Fantasia`, and
  `quick master (reform version)`. Other slots exposed real errors including
  clipped first characters, separators, appended UI text, Japanese glyph
  substitutions, and low confidence. These are diagnostic observations, not
  accepted labels or calibrated recognition.
- The same recording supplied `ABSOLUTE EVIL` in a result at source PTS 190000 and in right-list
  slot 10 at PTS 110000. The intact `BSOLUTE EVIL` suffix required translation only, not scaling;
  a neutral foreground-mask comparison measured intersection-over-union 0.9561805101373446. This
  supports shared thin-title glyph rasterization for that observation but does not establish
  universal texture identity. Visual inspection separately showed a different large selected-title
  renderer, the selected right row shifted into the generic crop's left edge, and long titles hidden
  by the list UI at the right edge. ADR 0016 therefore admits only stationary non-selected rows as
  provisional shared-title evidence while preserving result-only holdout.
- Against active catalog `65a8e164f3cb28e20c09114eecc3eb7200d32ed7c7383c7c04432053b78411eb`,
  the registered dictionary encoded 4,756 of 4,810 non-search variants from 2,548 songs. All songs
  had at least one non-search variant, but 54 variants were rejected. In-game display names accounted
  for 37 rejections: 10 contained unsupported characters and 27 exceeded the graph's 40 CTC
  timesteps. Official display, e-amusement CSV, and alternate display contributed 5, 1, and 11
  additional rejections respectively. The retained ignored audit is diagnostic and no variant was
  removed from the inference domain.
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
  complete active-catalog dictionary coverage, scroll stability calibration, temporal agreement, independent
  screen context, and accepted title semantics remain unimplemented.
  A scorepeek-owned candidate export and its tensor contract have been verified, but outside
  replay rejected that fine-tuned checkpoint in favor of the mapped initializer. Direct export and
  ONNX replay of that initializer, catalog-update recognition replay, event daemon, and the integrated live
  flow remain unvalidated. The observed 649 ms CPU process and inference time
  is a single warmed development-host measurement, not a performance gate.
- The title-model requirements and preparation boundaries have synthetic regressions and one real
  private-artifact execution. The music-list contract has one artifact-bound real-row verification,
  a complete measured pair artifact, and a
  review disposition for all 7,977 exact-pixel groups, but 2,959 groups covering 3,192 occurrences
  intentionally remain unknown for vertical motion, unobservable locked-row color, possible
  right-clipping, or redacted secret-title pixels. The presentation clusters are private
  single-recording evidence, not a supported classifier or reusable automatic threshold. The
  scorepeek-owned dictionary, title-disjoint preparation, mapped training initializer, bounded
  fine-tuning candidate, export, parity reference, and replay have been produced from private
  artifacts. Model selection now scores the complete current 1,879-song candidate set directly
  from the CTC probability tensor; strict and space-insensitive open-text matches are diagnostics,
  not acceptance metrics. Re-evaluation of the initializer and six earlier standard,
  channel-maximum, and targeted checkpoints found initializer validation/evaluation crop decisions
  at 327/329 and 369/370. Every candidate that improved validation reached 328/329 but remained
  369/370 on evaluation; the older v1 pilot regressed validation to 325/329. The initializer's
  correct runner-up margins remained separated from its incorrect margins across these splits,
  while each fine-tuned candidate introduced overlap. The initializer fully resolves 109/111
  validation songs and 132/133 evaluation songs across all available crops. Those title-disjoint
  subsets are generalization diagnostics rather than the finite-corpus model-selection oracle. A
  current-catalog candidate artifact at SHA-256
  `36fa8d3fff16eefb27cadea2f16c4395b6d40ca4ea1505d8e9c119ca84748e1a`
  independently regenerated the `ceabe293` search space. The v2 catalog-selected one-step pilot at
  manifest SHA-256 `d2d84a29bb9164caf9fd4cc7ba45c384db17f187771472db5277b26f4da8690a`
  preserved every previously correct validation crop and raised fully resolved validation songs
  from 109 to 110. Replay manifest SHA-256
  `971df3c809e40fd2592151228c9db7b1c5c7180acdbcc0c20213c1b4701a5035` remained
  132/133 on evaluation. A complete 3,061-crop, 1,119-song census then re-evaluated the initializer
  and all six retained pilot checkpoints against all 1,879 current-catalog candidates. The private
  census manifest at SHA-256
  `bfeae677001efd1dc8beeb7cafeca1eea73a98bfa4c71be89a8edc360b3259ed` measured the initializer at
  1,112/1,119 fully recognized songs. Pilot v1 reached 1,113; pilot v2, pilot v3, channel-max v1,
  catalog v1, and catalog v2 each reached 1,114. Those five candidates preserved every initializer-
  recognized song and additionally resolved `If` and `■□模様`; the earlier evaluation-only rejection
  was therefore not valid for the finite-corpus goal. The latest fully current-bound catalog-v2
  checkpoint is the private winner at SHA-256
  `a027393671e2a6dd65fd8009e7692c848c9dc42c1d8931aeee99568282d46a50`, leaving five corpus songs
  incomplete: `ΕΛΠΙΣ`, `〆`, `Ｘ↑Ｘ↓`, `ＰＡＳＴＥＬＩＳＭ`, and `∀`. Correct and incorrect margins
  overlap, so this census does not calibrate the live rejection thresholds. The winner's observed
  65-timestep, 18,725-class CTC output is not yet a selected exported runtime graph.
  The 3,061 catalog-bound
  music-list labels are provisional only: the 2,611 automated associations and 450 visual
  associations do not establish accepted holdout truth, a stability threshold, or a release gate.
  No eligible standard group remains unknown in this recording. The versioned v2 comparison key
  and cross-language Unicode 17 tests implement exact-first, bounded ASCII/fullwidth fallback without general
  compatibility normalization. The private 1,879-candidate audit found zero cross-song fallback
  collisions and 47 fallback keys with multiple display values for one song; training-label
  association remains unknown when the resolved tier does not identify one display value. Exact
  matches therefore cannot be invalidated by a broader fallback collision.
- The live `ObservedFrame`/domain-normalizer/`CanonicalFrame` runtime boundary,
  model-bundle promotion, and last-known-good model rollback remain
  unimplemented. Recognition accepts only digest-bound offline canonical
  extraction artifacts; it has no direct observed-frame input.
- The persistent systemd installer's custom unit-path linking, timer enablement,
  and unified disable path were reviewed but not deployed to the user's actual
  configuration. Only the non-persistent transient user-manager path was run.
- Bazzite Portal, Gamescope, OBS, GPU, lifecycle, performance, and soak gates
  remain target-machine-only and unrun.
- Two real OBS/vkcapture game recordings have passed copyless import, generation sealing, and local
  full-hash verification. The first has also passed reimport,
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
- Offline Python 3.12.13, uv 0.11.7, PaddleOCR 3.7.0, PaddlePaddle CPU 3.3.1,
  Apache-2.0 `paddle2onnx` 2.1.0, `scikit-image` 0.26.0, `albumentations` 2.0.8,
  `albucore` 0.0.24, `lmdb` 2.3.0, `rapidfuzz` 3.14.5, and `unicodedata2` 17.0.1,
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
- ADR 0015 allows provenance-bound catalog strings and real title crops in
  private provisional development generations. Any model containing such data
  remains non-distributable until the planned upstream permissions and license
  evidence cover every contributing generation.

## Next executable task

Keep the catalog-v2 checkpoint as the current private winner. Improve music-select recognition from
the standard domain first. Its five incomplete corpus songs consist of three train songs (`〆`,
`ＰＡＳＴＥＬＩＳＭ`, and `∀`) with eight failed crops, one validation song (`Ｘ↑Ｘ↓`), and one evaluation
song (`ΕΛΠΙΣ`). Run the smallest targeted candidate using only those eight already independent train
crops; do not train on the validation or evaluation failures. A candidate may advance only when the
complete 1,119-song census preserves every fully recognized song and strictly grows that song set
beyond 1,114. Validation, evaluation, strict text, and comparison-key counts remain diagnostics and
may move independently; no split alone is the finite-corpus selection oracle. Runner-up and absolute
thresholds remain uncalibrated until negative, transition, and similar-title live inputs exist. The
channel-maximum transform matches the best coverage but is not preferred over the identity-domain
catalog-v2 checkpoint, is not a default, and is not a live-only correction. Keep the 24
INFINITAS-blue and 299 LEGGENDARIA-purple groups out of training until their labels are established
independently of the recognition output; evaluate any future color correction under the same
explicit transform ID in training and replay.
Keep the 351 locked/unobservable, 438 possible right-clips, vertical-motion, selected, obscured,
and redacted-secret groups quarantined until their separate correction or completeness evidence
exists. After a music-select candidate clears the provisional split, export it through the
registered Paddle/ONNX path and replay the graph before runtime selection. Then collect result
observations with distinct titles; the current two result screens both contain `ABSOLUTE EVIL` and
cannot establish title-disjoint result accuracy or thresholds. Do not
tune recognition thresholds from the current two recordings, promote diagnostic commands into
accepted recognition, recognize bare PPM or `ObservedFrame`, auto-download a runtime model, or
treat the OBS profile, current ROIs, confidence, timing, or diagnostic thresholds as supported.

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
