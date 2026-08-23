# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it.
Uncommitted working-tree state is outside this checkpoint. Implementation history belongs in Git;
the roadmap and long-lived decisions remain in `docs/plan.ja.md` and `docs/decisions/`.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 offline canonical-frame and recognition spike: **in progress**.
- Current execution focus: introduce a profile-bearing frame boundary only from an admitted
  calibrated Gamescope lease, without exposing unnormalized frames to recognition.

## Included deliverables

### Catalog and private corpus

- Strict Tachi, Textage, and dqn acquisition, parsing, provenance, deterministic federation,
  quarantine, last-known-good activation, bounded private caches, and durable catalog snapshots.
- `scorepeek catalog sync` plus manual, persistent user-systemd, and transient scheduling routes.
  The CLI retains actionable credential-free adapter, transport, cache, and store error causes.
- Separate offline-only `scorepeek-corpus` tooling for bounded content-addressed ingest, FFV1
  packet-order probing and extraction, complete-label authoring, generation sealing, replay indexes,
  split isolation, dataset preparation, and S3-compatible transfer. Runtime code has no corpus or
  Python training dependency.
- Private frames, labels, recordings, source snapshots, generated catalogs, models, and environment
  artifacts remain outside the repository. Committed fixtures are synthetic or opaque and
  non-personal.

### Canonical recognition and diagnostics

- Fixed contiguous RGB8 1920x1080 canonical-frame contract with one shared layout, fail-closed
  result/music-select crops, contextual title recognition, and selection-song context.
- PP-OCRv6 small native-dynamic is the selected title observer. Registered model bundles,
  preprocessing, CTC decoding, exact-first comparison keys, catalog search, and private replay
  tooling are digest-bound and reproducible. Custom training/export is deferred until integrated
  evidence isolates a residual that requires it.
- Application-owned bounded QOI diagnostic runs with non-blocking producer handoff, a dedicated
  writer, strict replay, explicit partial/degraded coverage, crash recovery, retention, read-only
  controls, and create-only export. Pixels and recognition facts remain separate from public result
  surfaces.

### Gamescope capture and calibration

- Linux x86-64 native PipeWire build uses the mise-pinned PipeWire 1.6.8 SDK and pkgconf 3.0.1;
  the host supplies the PipeWire runtime, C compiler, and matching libclang resources.
- The Gamescope provider discovers exactly one default-remote `gamescope` `Video/Source`, returns an
  explicitly uncalibrated lifetime lease, latches selected-node loss without fallback, and keeps
  provider and receiver failure ownership distinct.
- The common receiver accepts only bounded raw BGRx, disables conversion and reconnection, retains
  one owned latest frame, detects caps/memory/stride drift, and tears down before the provider.
  Callback work is limited to bounded state changes and the required pixel copy.
- Live and repeated-lifecycle gates expose bounded typed facts and aggregate counters without pixels
  or arbitrary PipeWire properties. Uncalibrated frames cannot enter recognition or canonical
  diagnostic recording.
- Create-only calibration sampling records exact bounded environment, Gamescope version, backend,
  output size, nested size/refresh, scaler/filter, complete observed BGRx contract, frame digest,
  receiver sequence, monotonic receive time, and typed capture facts. Hashing, serialization,
  filesystem publication, and fsync occur after receiver/provider shutdown.
- `scorepeek-gamescope-profile-binding-v1` is canonical JSON selected by an independent SHA-256. It
  binds calibration evidence, exact Gamescope provenance/configuration, full observed contract,
  opaque profile digest, fixed canonical contract, normalizer implementation, and explicit rational
  geometry. Parsing and contract comparison are pure, bounded, filesystem-free, and fail closed.
- Binding validation requires provider output width/height to equal the observed video width/height;
  internally valid but cross-field-inconsistent artifacts are rejected as invalid profiles.
- A new Gamescope acquisition can carry explicit launcher/operator-owned session provenance. It is
  promoted to `CalibratedGamescopeLease` only when every provenance field and every negotiated
  video/memory/stride field exactly match the selected immutable binding. Missing or drifting data
  fails closed, and the rejected receiver remains explicitly shut down by its owner.
- The bounded capture diagnostic sink receives exactly one typed, value-free admission fact. The
  live `gamescope-binding-admission-gate` report exposes stable acceptance/rejection categories and
  bounded capture facts, but not binding bodies, session strings, paths, pixels, arbitrary
  PipeWire properties, or raw PipeWire errors. The calibrated type does not yet expose frames.
- The explicit normalizer maps BGRx through source rectangle
  `x=26/3, y=0, width=7616/3, height=1428` using the registered half-pixel/Q11 linear rule into an
  unbound RGB8 1920x1080 candidate. There is no automatic measurement, border detection, profile
  generation, or fallback.

## Verified checkpoint evidence

- Controlled Gamescope `3.16.19-128-g7282613+` session used explicit SDL backend, output
  2556x1428, nested 1920x1080 at 120 Hz, scaler `auto`, and filter `linear` with an independently
  generated marker application.
- The retained private development-machine sample was independently rehashed and reviewed:
  - manifest SHA-256: `faef6770fae4fa3e21ffd069cb274e45d8ae3054bc75b69038ebbef3f574c6d0`
  - raw frame SHA-256: `f5a6fea1f9e2e7eec214fef75b70bef7b55f61961dd00025abc36782090e8753`
  - binding artifact SHA-256: `aad69103654afb3773198eebcb888db04ce86834c619f8781cc2f6c28405b2b2`
  - capture profile SHA-256: `e0a27efb0119a8711ada7b3ddc6811fc9fb669b7d1ce7abc4cbc89562365414e`
- Known edge and interior markers independently reproduced the same rational geometry and canonical
  result. The artifact and captured pixels remain in operator-owned local state, not the repository.
- A fresh controlled marker session admitted that private binding after observing 2556x1428 BGRx
  MemFd with 10,224-byte stride. A separately restarted session with only declared nested refresh
  changed from 120 to 119 was rejected as `profile_nested_refresh_mismatch`; both runs recorded one
  value-free admission fact and completed receiver/provider shutdown with no dropped facts. A
  second capture attempted without restarting the static marker timed out before admission and is
  not acceptance/rejection evidence.
- Development-host Gamescope/vkcube gates have exercised actual negotiation/frame reception,
  selected-node loss, latest-frame overwrite under consumer pressure, receiver-before-provider
  shutdown, and 100 repeated acquire/start/stop lifecycles. A separately operator-started session
  negotiated 2556x1428 BGRx MemFd with 10,224-byte stride at approximately 60 fps. The pixel-free
  gate does not prove which application content was displayed.
- Catalog live sync, isolated scheduling/locking, private corpus import/replay, diagnostic replay,
  official-model execution, and native PipeWire build have each passed their dedicated development
  gates. These do not substitute for target-machine capture, recognition, or performance evidence.
- Repository validation at this checkpoint includes formatting/static checks, workspace all-target
  clippy with warnings denied, Rust unit/integration tests, 75 offline OCR tests, dataset E2E, and
  native PipeWire build verification.

## Unverified boundaries

- No profile-bearing `ObservedFrame`, live `DomainNormalizer` handoff, or recognition input exists.
  The calibrated lease deliberately has no frame-production API; negotiated caps alone never
  establish profile identity.
- Session provenance is explicit launcher/operator input, not an automatic observation of the
  Gamescope process. Process discovery or attestation is not implemented or claimed.
- INFINITAS content/geometry, target play-machine output, 4K, FSR/NIS, Reshade, HDR, Portal, and OBS
  are separate uncalibrated domains. The development-machine profile is not a pixel reference.
- OBS/obs-vkcapture coexistence, PipeWire daemon disconnect, stream loss distinct from node loss,
  source recreation, long soak, FD/thread/RSS convergence, frame age, CPU/memory/copy/GPU/power
  cost, game p99 frametime, and OBS lag remain unverified.
- The existing result and music-select recognition work is offline/private evidence. There is no
  integrated live recognition, accepted field gate, event daemon, target-host performance gate, or
  supported capture profile.
- Current recordings and provisional labels do not establish title-disjoint result accuracy,
  calibrated thresholds, result dwell, miss denominator, deduplication, or release accuracy.
- Persistent scheduler installation was verified in isolation but not applied to the operator's
  actual configuration. Real S3 credentials/provider behavior and remote bucket lifecycle remain
  untested.

## Approval and authority boundaries

- New dependencies require prior approval with purpose, version/license, alternatives, and
  runtime/bundle/host/reproducibility impact.
- Captured frames, game/player data, complete labels, raw catalog inputs, generated catalogs, OCR
  models, credentials, and environment-specific artifacts must not be committed.
- External-source use remains governed by `docs/sources.md`; no source requiring new permission may
  be enabled without it.
- Push, release, deployment, persistent host configuration, real remote-storage changes, Portal/OBS
  setup, and target-machine changes require explicit user authority.

## Next executable task

1. Define the profile-bearing frame boundary required by ADR 0029 so only an admitted calibrated
   lease can produce it. Preserve the immutable capture-profile and normalizer identities without
   exposing an uncalibrated frame path.
2. Apply the binding-selected domain normalizer to produce the fixed canonical RGB8 1920x1080
   contract, with generation/profile rollover failing closed. Keep this handoff structurally
   separate from recognition until its lifecycle and pixel contract are independently tested.
3. Extend bounded diagnostics only with the minimum typed frame/normalization lifecycle facts needed
   to distinguish admission, normalization, rollover, and shutdown. Do not record artifact bodies,
   session strings, paths, pixels, arbitrary properties, or raw PipeWire errors.
4. Re-run controlled marker canonicalization, receiver/provider lifecycle, and complete repository
   validation, then independently review the frame ownership and profile-generation boundary.

Do not proceed to OCR-only tuning, automatic calibration, Portal/OBS fallback, soak/performance, or
support claims until this admission boundary and its evidence are complete.

## Stable milestone map

| ID | Milestone | State |
| --- | --- | --- |
| M0 | Independent design, bootstrap, and target inventory | complete |
| M1 | Catalog federation and activation | complete |
| M2 | Private corpus, synthetic fixtures, and replay tooling | complete |
| M3 | Common PipeWire receiver and Gamescope calibrated observed-frame profile | in progress |
| M4 | Shared canonical layout, official recognizer, and contextual recognition | in progress |
| M5 | Supported capture-profile evaluation and default selection | pending |
| M6 | Fail-closed integrated field recognition and cross-field validation | pending |
| M7 | Live diagnostics, versioned events, and NDJSON daemon | pending |
| M8 | Integrated holdout and Bazzite release gates | pending |
