# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it.
Uncommitted working-tree state is outside this checkpoint. Implementation history belongs in Git;
the roadmap and long-lived decisions remain in `docs/plan.ja.md` and `docs/decisions/`.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 offline canonical-frame and recognition spike: **in progress**.
- Current execution focus: define the application-owned live field-observer execution boundary for
  model/catalog inputs already fixed by the immutable session, without blocking capture or letting
  diagnostic outcomes affect field observations.

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
  PipeWire properties, or raw PipeWire errors.
- A nonzero application-owned `CaptureGeneration` is fixed at admission. Only the admitted lease
  can turn its bounded latest raw frame into an `ObservedFrame` carrying generation, capture-profile,
  and normalizer identities; raw pixels remain inaccessible through that type.
- The same lease applies only its binding-selected fractional geometry. Generation, profile, or
  normalizer mixing fails closed before normalization. Successful output is a structurally separate
  `NormalizedCanonicalFrame` containing contiguous RGB8 1920x1080 pixels and immutable source
  sequence/timing/profile/normalizer/generation evidence; it is not a recognition `CanonicalFrame`.
- Capture diagnostics record at most the first normalization success and first normalization
  failure per admitted lease, with only source sequence and stable typed status/error. Per-frame
  diagnostic traffic, pixels, paths, binding bodies, and environment/session strings are absent.
- `gamescope-canonical-frame-gate` performs bounded binding selection, acquisition, admission,
  one-frame normalization and ordered shutdown. Its result contains only identities, generation,
  source sequence, canonical RGB8 digest, and bounded capture facts; it does not retain pixels.
- Application `LiveCanonicalFrame` values can now be created only by consuming a
  `NormalizedCanonicalFrame`. The canonical `Box<[u8]>` is moved into an `Arc` owner without a
  second RGB copy; diagnostic queue offers clone only that owner. The old public constructor that
  accepted caller-invented generation, profile, normalizer, timing, and pixels no longer exists.
- `gamescope-diagnostic-handoff-gate` binds one explicit capture generation to one application-owned
  diagnostic descriptor, normalizes a bounded live frame sequence, offers every frame before any
  recognition result, and reports cadence, opt-out, queue, worker, completion, and manifest outcomes
  separately from capture facts. The gate derives capture-profile and normalizer identities from
  the selected binding rather than CLI values. Provider, receiver, normalized frames, and diagnostic
  run bounds share the provider-lease monotonic origin. The gate shuts down receiver/provider before
  finalizing the diagnostic run, so a teardown failure is retained as an error manifest rather than
  an immutable successful run.
- `LiveRecognitionObservation` borrows the same `LiveCanonicalFrame` owner used by diagnostics and
  applies the embedded result/music-select screen predicate without another full-frame RGB copy.
  It cannot detach from or invent live generation/profile/normalizer evidence. The pure RGB8
  predicate has no provenance or acceptance authority by itself.
- The backward-compatible `gamescope-recognition-handoff-gate` keeps the earlier diagnostic-only
  command intact. It requires the descriptor's actual embedded layout digest before acquisition,
  and now runs through an application-owned `LiveRecognitionSession`. The session validates the
  complete immutable descriptor, offers canonical evidence before inspection, rejects mismatched
  frames before recognition, and records each typed screen observation through the same worker.
  Its stable identity binds capture generation, profile, normalizer, layout, catalog, model, and
  runtime. Explicit transition records the next identity in the old run, finishes that run, and
  only then starts the replacement session. Recording rejection, opt-out, queue loss, or store
  failure remains separate from the screen result.
- Offline crop export and the live session now share one synchronous, deterministic,
  filesystem-free RGB field-routing API with screen-specific required fields. `result` carries
  title, artist, difficulty, level, notes, and current score; `music_select` carries central title,
  artist, selected chart, and active-list title. This removes the earlier supplemental-context-only
  live shape that omitted song title. `unknown` cannot construct field inputs. Live inputs borrow
  the admitted canonical owner and carry no OCR, song, accepted-field, or event authority. Fields
  whose layouts are not yet measured, including play mode, are not represented as empty optional
  crops. The live owner is opaque: only the session can join crops to the admitted frame, and
  callers receive screen-specific borrowed views. Model bundle I/O and inference are not part of
  this checkpoint.
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
- Two independently restarted controlled marker sessions used capture generations 1 and 2. Both
  admitted the same private profile, normalized source sequence 1, and produced canonical RGB8
  SHA-256 `4ea79fa76f6f87b5328222db1690d6f403fc6fa652411d932aa9247e7ea0d084` with successful ordered
  shutdown and no dropped facts. Direct RGB8 conversion of the source PNG is not a compositor-path
  pixel reference and produced a different digest; it is not used as the acceptance oracle.
- A controlled two-frame marker animation retained the known edge and center geometry while making
  the compositor submit multiple frames. Capture generation 16 normalized sequences 1 through 13;
  the live bridge offered all 13, admitted three at the fixed 1,000 ms cadence, and published a
  complete three-frame diagnostic manifest with no capture-fact, queue, worker, or binding drop.
  Its manifest SHA-256 was `1fa2e90095b793601c91e542cce9f9411a466758d160706df42f3203567b5c5a`.
  Its first recorded frame began at 38 ms and the run ended after ordered receiver/provider shutdown
  at 2,629 ms on the same provider-lease monotonic clock.
- A separately acquired generation 17 normalized sequences 1 through 7 with recording disabled.
  All seven offers returned `disabled`, no diagnostic manifest or frame was created, capture and
  shutdown still succeeded, and the supplied diagnostic root remained empty. Synthetic catalog and
  model fixture digests in these controlled runs identify only that no lookup or inference ran;
  they are not catalog/model validation evidence.
- Capture generation 18 ran the controlled geometry marker through the live screen predicate. All
  13 normalized frames remained `unknown`, all 13 typed `inspect_recognition` facts entered the same
  complete diagnostic run, and no frame/fact queue or worker drop occurred. Its manifest SHA-256
  was `8f7608dde39c9325c75982d6c40e2a167bcc85985aed4601ae5e4e216b40baf7`.
- Separately generated, game-asset-free color rectangles exercised both positive predicate classes
  through Gamescope generation 20: 2 `result` and 11 `music_select` observations from 13 normalized
  frames, with 13 matching typed facts and a complete manifest SHA-256 of
  `354f672fe658558ca4d9b8ba3d281af4f8a9e20f5264fb97bf3a336109a69a51`.
  These synthetic predicates prove live routing, not INFINITAS layout or recognition accuracy.
- Generation 19 repeated the marker predicate with recording disabled: all 6 normalized frames
  remained `unknown`, all frame and fact offers returned `disabled`, and the diagnostic root stayed
  empty without changing the predicate result.
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
  native PipeWire build verification. Focused session tests also verify descriptor/layout rejection,
  frame-generation rejection, diagnostic opt-out non-interference, and manifest-backed ordered
  binding rollover.
- Focused routing tests exercise synthetic result and music-select inputs, title-bearing exact
  screen-local field sets, retained live owner identity, diagnostic opt-out, and structural unknown
  exclusion. Existing offline result, music-select, and integrated-context export tests pass through
  the same routing function.

## Unverified boundaries

- No live model/catalog inference, field observation, accepted result, context reducer, or event
  output consumes the routed inputs; this checkpoint stops at bounded screen-local RGB crops under
  the immutable application lifetime.
  Offline `CanonicalFrame` extraction evidence still cannot fabricate the live generation/profile/
  normalizer owner or enter this live gate.
- The bounded CLI gate is not an ordinary long-running application loop. Live queue-full or worker
  loss was not forced on the development machine; bounded unit tests cover queue drop, worker loss,
  generation rejection, opt-out, and diagnostic non-interference. Target-host cost remains unknown.
- Session provenance is explicit launcher/operator input, not an automatic observation of the
  Gamescope process. Process discovery or attestation is not implemented or claimed.
- INFINITAS content/geometry, target play-machine output, 4K, FSR/NIS, Reshade, HDR, Portal, and OBS
  are separate uncalibrated domains. The development-machine profile is not a pixel reference.
- OBS/obs-vkcapture coexistence, PipeWire daemon disconnect, stream loss distinct from node loss,
  source recreation, long soak, FD/thread/RSS convergence, frame age, CPU/memory/copy/GPU/power
  cost, game p99 frametime, and OBS lag remain unverified.
- The existing result and music-select field recognition work is offline/private evidence. There is
  no integrated live field observer, accepted field gate, event daemon, target-host performance
  gate, or supported capture profile.
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

1. Define an application-owned field-observer execution boundary that consumes only
   `LiveScreenRgb8Crops`, loads the descriptor-bound model/catalog inputs once per immutable
   session, and does not perform model or filesystem I/O in the PipeWire callback.
2. Bind any field observation, song decision, suppression, and accepted result to that same run;
   diagnostic enqueue/drop/opt-out must not alter those recognition outcomes.
3. Verify real field routing with separately authorized private INFINITAS evidence before claiming
   layout, OCR, accepted-field, target-host, or support results.

Do not proceed to OCR-only tuning, automatic calibration, Portal/OBS fallback, soak/performance, or
support claims until live field routing and its immutable run-lifecycle evidence are complete.

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
