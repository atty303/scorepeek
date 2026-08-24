# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it.
Uncommitted working-tree state is outside this checkpoint. Implementation history belongs in Git;
the roadmap and long-lived decisions remain in `docs/plan.ja.md` and `docs/decisions/`.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 offline canonical-frame and recognition spike: **in progress**.
- Current execution focus: the corpus recording has passed the value-bearing result-song
  recognition simulation for all three reviewed episodes, and a normal foreground Gamescope
  session now reuses that post-canonical path. The retained Wayland result QOI proved that the two
  result-panel edge crops were one row low; the measured coordinate correction classifies that exact
  frame without lowering the threshold and passes the complete three-episode simulation. Foreground
  evidence is now bounded for hours-long use and selected result frames can retain paired exact raw
  BGRx for later transform replay. A later foreground run exposed startup frames that satisfied the
  two palette-wide music-select predicates. The corrected layout now additionally requires the
  measured fixed `MUSIC SELECT` label and rejects the retained startup evidence offline. A transform
  comparison command and a prospective ordinary live result under the corrected layout remain next
  boundaries; another play is not required to repair either observed recognition failure.
  Release accuracy, event authority, target-host performance, and support remain later gates.

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
  result/music-select crops, contextual title recognition, and selection-song context. Music-select
  presence requires the fixed label structure in addition to the existing header and level-column
  palette evidence.
- PP-OCRv6 small native-dynamic is the selected title observer. Registered model bundles,
  preprocessing, CTC decoding, exact-first comparison keys, catalog search, and private replay
  tooling are digest-bound and reproducible. Custom training/export is deferred until integrated
  evidence isolates a residual that requires it.
- Application-owned bounded QOI diagnostic runs with non-blocking producer handoff, a dedicated
  writer, strict replay, explicit partial/degraded coverage, crash recovery, retention, read-only
  controls, and create-only export. Foreground runs keep a 12-second unknown tail, result/transition
  evidence and low-frequency baselines; only the first partial-result or a known-screen transition
  may pair exact raw BGRx with its same-sequence canonical QOI. Pixels and recognition facts remain
  separate from public result surfaces.

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
- The bounded capture diagnostic sink receives exactly one compact typed admission fact. The
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
- Application `BoundCanonicalFrame` values can now be created only by consuming a
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
- `RecognitionObservation` borrows the same `BoundCanonicalFrame` owner used by diagnostics and
  applies the embedded result/music-select screen predicate without another full-frame RGB copy.
  It cannot detach from or invent live generation/profile/normalizer evidence. The pure RGB8
  predicate has no provenance or acceptance authority by itself.
- The backward-compatible `gamescope-recognition-handoff-gate` keeps the earlier diagnostic-only
  command intact. It requires the descriptor's actual embedded layout digest before acquisition,
  and now runs through an application-owned `RecognitionSession`. The session validates the
  complete immutable descriptor, offers canonical evidence before inspection, rejects mismatched
  frames before recognition, and records each typed screen observation through the same worker.
  Its stable identity binds capture generation, profile, normalizer, layout, catalog, model, and
  runtime. Explicit transition records the next identity in the old run, finishes that run, and
  only then starts the replacement session. Recording rejection, opt-out, queue loss, or store
  failure remains separate from the screen result.
- Offline crop export and the live session now share one synchronous, deterministic,
  filesystem-free RGB field-routing API with screen-specific required fields. `result` carries
  title, artist, clear type, difficulty, level, notes, and current score; `music_select` carries central title,
  artist, selected chart, and active-list title. This removes the earlier supplemental-context-only
  live shape that omitted song title. `unknown` cannot construct field inputs. Live inputs borrow
  the admitted canonical owner and carry no OCR, song, accepted-field, or event authority. Fields
  whose layouts are not yet measured, including play mode, are not represented as empty optional
  crops. The live owner is opaque: only the session can join crops to the admitted frame, and
  callers receive screen-specific borrowed views. Model bundle I/O and inference are not part of
  this checkpoint.
- The application now has a generic field-observer execution boundary under ADR 0030. It derives
  an immutable session binding from the complete descriptor, calls one application loader before
  worker startup, and accepts only opaque crops carrying the same run ID and full binding. A
  capacity-two `try_send` queue keeps observer execution off the capture loop. Worker-produced
  results bind sequence, monotonic interval, screen, and session provenance independently of the
  observer output. The same global capacity bounds accepted but unconsumed results after queue
  removal. Queue full, outstanding-result limit, worker loss, abandoned results, and bounded finish
  timeout remain typed; timeout does not claim the residual thread has terminated, and the
  production-worker token remains held through observer teardown.
- ADR 0031 adds the production resource loader used by that boundary. It matches the active catalog,
  registered PP-OCRv6-small model, and fixed CPU runtime artifact to the immutable run digests,
  verifies the complete bundle, and retains the catalog, dictionary, and one ONNX session before
  worker startup. The runtime exposes bounded-crop open-text observation without field or song
  authority. The read-only load gate transfers those resources into the production field worker
  and requires bounded teardown without submitting crops. Its JSON report contains only typed
  status and selected digests; it does not add resource bodies, paths, environment strings, or
  pixels as fields, while ordinary typed error causes remain actionable on stderr.
- ADR 0032 as narrowed by ADR 0036 adds the production screen-field observer and complete
  screen-specific output shapes. Result observations contain title, artist, and clear-type text plus explicit not-implemented states for
  difficulty, level, notes, and current score. Music-select observations contain central title,
  artist, and active-list title text plus an explicit not-implemented selected-chart state. There
  are no title-only, artist-only, supplemental, or optional partial screen outputs. Failure of any
  text field returns a typed whole-screen error naming the failed field. The corresponding bounded
  field-count fact records screen, fixed field counts, and an optional typed failed-field ID;
  diagnostic disablement or rejection does not change the bound observation result. ADR 0037 now
  requires the application-owned recognition artifact to retain bounded exact OCR strings and
  candidate evidence rather than treating this compact fact as the complete evidence surface.
- ADR 0033 adds one application owner for the immutable recognition session and matching registered
  field worker. The exact observer loads before the diagnostic-backed recognition run opens.
  Inspection returns screen, non-blocking field-submission, and diagnostic outcomes separately;
  unknown screens do not submit. Opaque owner/pending tokens reject another run before consuming a
  result. A pending result is consumed at most once, completed output is recorded through the
  existing compact field fact, and completed or disconnected handles are terminal after their
  first result. An exact capacity-two ledger binds abandoned results to their source sequences;
  lifecycle timeout or worker loss is unbound rather than attributed to an invented frame. These
  degradations do not replace recognition. Finish closes the field worker before finalizing the
  diagnostic run.
- ADR 0034 adds an immutable full-catalog candidate domain. It retains every active catalog song in
  stable ID order and computes separate minimum edit distance and exact integer maximum normalized
  similarity for each observed result title/artist or music-select central-title/artist/active-list
  title. Raw, exact comparison-key, and domain-unique folded forms are compared without ranking,
  truncation, intersection, threshold, accepted field, song decision, temporal state, suppression,
  diagnostic side effect, or event authority. Folded observations compare only with admitted
  domain-unique folded candidate forms; a search-term-only song fails domain construction with its
  typed ID instead of panicking or disappearing. Unimplemented non-text fields remain explicit
  inputs and do not fabricate scores.
- ADR 0035 connects that domain to the registered production observer and a bounded Gamescope gate.
  The observer constructs the domain once from the already admitted active catalog, then returns
  the complete field set and all-song evidence together without ranking or acceptance. The gate
  owns capture, normalization, classification, non-blocking field submission, selected pending
  completion, receiver/provider shutdown, field-worker finish, and diagnostic finalization under
  one immutable descriptor. Success requires at least one completed candidate set. Its current
  compact JSON contains typed status and bounded screen/worker/candidate counts; that output is an
  execution-gate result, not sufficient recognition evidence.
- ADR 0036 adds a recording canonical-source adapter beside the Gamescope adapter. A create-only
  profile binds the original recording, recording/source manifests, probe, reviewed coverage label,
  complete canonical extraction, normalizer/layout/resources, delivery pacing, diagnostic sampling,
  and ordered result windows with exact expected `CLEAR TYPE` text. The extraction source manifest
  must match the recording manifest, and profile episodes exactly cover every strictly parsed label
  result. Every extraction frame enters the same recognition session, crop router, registered
  worker, and full-catalog domain used after live normalization. Result presence uses the fixed
  result header and two measured panel boundaries; background palette and result artwork are not
  predicate inputs. ADR 0036 itself grants no accepted field, song, event, live-support, or
  performance authority; ADR 0038 adds the later result-song resolver only.
- ADR 0037 supersedes the recognition-value suppression in ADR 0032, ADR 0035, and ADR 0036.
  Operator-owned local recognition artifacts must retain bounded exact OCR strings, a run-scoped
  exact catalog display/comparison string table with candidate references, song IDs, complete
  per-field candidate metrics, resolver decisions and reasons, and reviewed expected-versus-observed
  values. Compact command output and the future event API remain distinct sinks. Pixels stay in the
  bounded image store and are joined by identity rather than duplicated.
- ADR 0038 adds the fail-closed result-song resolver
  `scorepeek-result-song-title-primary-artist-corroborated-v1`. It requires nonempty title/artist,
  at least two catalog candidates, selected title edit distance at most one, title similarity at
  least `6/7`, runner-up title edit margin at least two, and selected-candidate artist similarity at
  least `2/5`. Artist corroborates the title-selected song rather than contributing to a combined
  rank. Every rejection is a typed unknown with candidate evidence when available.
- Recording profile v2 requires an exact expected `ScorepeekSongId` for every episode. The
  recognition simulation requires at least two exact expected song decisions and two exact
  expected `CLEAR TYPE` observations per episode, rejects a different accepted song immediately,
  and retains sequence/PTS, exact OCR, exact catalog strings, all per-field metrics,
  decisions/reasons, and expected values in a create-only bounded local artifact. Catalog JSON is
  capped at 16 MiB; observation NDJSON is capped at 256 MiB and 3,600 records; the manifest is
  created last after child sync.
- ADR 0039 adds `gamescope-result-recognition-gate` without removing the existing counts-oriented
  field gate. Completed registered observations move through a capacity-two non-blocking worker to
  the same exact-value serializer used by recording simulation. Observation schema v2 distinguishes
  recording source PTS from the live bound monotonic start/end interval. Queue full, worker loss,
  write failure, and five-second finish timeout are typed and cannot replace recognition. The new
  command passes only when at least one completed result resolution exists, every completed
  observation was enqueued, and a complete create-only manifest was produced. Its top-level status
  agrees with the CLI exit on artifact failure. A process-wide supervisor rejects another writer
  until a timed-out worker actually exits; that worker may finish an already-started publication,
  but the timed-out run stays failed. The existing counts gate retains its v1 report and the new
  command uses a distinct v1 schema. Compact JSON links by status/count/digest rather than duplicating its
  OCR, song IDs, catalog strings, candidate metrics, or decisions.
- ADR 0040 adds `scorepeek run gamescope`, which owns one admitted provider until the exact stdin
  control line `stop` or a typed terminal failure. It emits exact bounded field/resolver NDJSON,
  preflights an enabled private diagnostic root, records full numeric screen-predicate evidence for
  unknown as well as recognized screens, and finalizes the existing field, diagnostic, and
  recognition-artifact workers in order. Its control path does not signal Gamescope, INFINITAS, or
  the process group.
- ADR 0043 makes the foreground artifact practical for multi-hour sessions. Recognition evidence
  retains one representative result per interval, splits result observations separated by more
  than 30 seconds even without music-select, and keeps five-minute music-select samples, with
  compact candidate metric arrays in exact catalog order. Diagnostic QOI uses a bounded failure window;
  paired source bytes are limited by the existing 8 GiB run/store capacity and are not recorded
  continuously. Transient predicate cooling while the screen remains unknown does not start a new
  raw-source interval, and a rejected rolling-tail batch accounts for every selected sequence.
  New run-start documents use v2; exact canonical v1 documents remain readable as the former
  complete-cadence policy, so retained evidence cannot block a new writer. Bounded gates and
  offline simulation retain complete observations.
- The explicit normalizer maps BGRx through source rectangle
  `x=26/3, y=0, width=7616/3, height=1428` using the registered half-pixel/Q11 linear rule into an
  unbound RGB8 1920x1080 candidate. There is no automatic measurement, border detection, profile
  generation, or fallback.

## Verified checkpoint evidence

- Controlled Gamescope `3.16.19-128-g7282613+` session used explicit Wayland backend, output
  2556x1428, nested 1920x1080 at 120 Hz, scaler `auto`, and filter `linear` with an independently
  generated marker application.
- The retained private development-machine sample was independently rehashed and reviewed:
  - manifest SHA-256: `93fe9c0e80c545c585c60901ff776bd06d652bd0422385cd75c68757d11811f5`
  - raw frame SHA-256: `a9798ac8abdf03edeb28355a1d60d26ef2f79734767d27b316862e0ea2f57639`
  - binding artifact SHA-256: `c971ec19e1ed281a40ca43f0b5652f68b8d4eb7284f5725599a9920cc51c2a4a`
  - capture profile SHA-256: `6f01cfb3a5fe93f4cefde21ed0f358ca73db8c14d0e615de07ef1711bc4e38d6`
- An independent implementation of the registered half-pixel/Q11 normalizer reproduced canonical
  RGB8 SHA-256 `ad52c2d25cc997ed5fc82251bab56b78a8e632c4e02b3b11f93f27b1259a9d1e`
  from the retained raw marker frame. Against the independently generated source marker it had
  mean absolute error `0.235762`, with 2,059,655 of 2,073,600 RGB pixels exactly equal; known
  top-left, bottom-left, and center markers were exact. The artifact and captured pixels remain in
  operator-owned local state, not the repository.
- After the operator changed the Scroll rule for the exact Gamescope title so that its content
  surface, not only the outer window, is 1920x1080, a controlled marker session exposed matching
  1920x1080 BGRx PipeWire caps. The retained create-only sample negotiated MemFd with 7,680-byte
  stride, and its 8,294,400 raw bytes were byte-for-byte identical to an independent BGRx decode of
  the known marker. The resulting identity-geometry Wayland binding remains in operator-owned local
  state with manifest SHA-256 `9bd31b6f7b1f8096cb5f7ca8009189fd0d9c1b67aa84eaab227f3e3d05cb60f8`,
  raw-frame SHA-256 `8fc095df3bdf69ae346546ef57a74e2110fd1bf63952ed90a6f2873cf84bb631`,
  capture-profile SHA-256 `b96c359926d83ebed452fe5ea0b42b1a3cf5a377094913203da4f97dccd671c3`,
  and binding SHA-256 `7d0e226d525340d719ce1d699e4c691d8ad391fa9176904736fca9f22465812a`.
  A preceding 1916x1076 sample exposed the unintended border subtraction and was moved to the
  recoverable desktop trash after the rule was corrected; it was not used to author a binding.
- A fresh controlled marker session admitted the replacement private binding after observing
  2556x1428 BGRx MemFd with 10,224-byte stride. A separately restarted session admitted the same
  profile under capture generation 23 and normalized source sequence 1 to canonical RGB8 SHA-256
  `074a3d849fdc2d09455a4c37f8a210d72b83f73ac2871f2f76e689b3a06bb427`, with ordered shutdown and
  no dropped diagnostic facts. The animation phase can differ from the retained calibration frame,
  so the independent retained-frame marker comparison, not digest equality across phases, is the
  pixel oracle.
- An ordinary generation-24 Wayland session admitted the same private binding and retained 263
  exact live field observations. Every retained observation was `music_select`; the largest
  observation gap ran from 171,486 ms to 311,216 ms across gameplay and the result, and no live
  `result` observation was produced before music-select observations resumed. The run therefore did
  not pass live recognition. Its configured diagnostic root was absent and the recorder degraded
  to `store_unavailable`; abrupt Ctrl+C did not finalize a terminal report or manifest and is not a
  valid shutdown result.
- A later ordinary Wayland foreground run admitted capture generation 26 and matched the complete
  2556x1428 BGRx MemFd/10,224-byte-stride binding before INFINITAS terminated for an unrelated
  application failure. Before termination it received and normalized 247 frames. The operator had
  not entered music selection, but the old two-color predicate submitted 19 false music-select
  observations. Exact retained sequence 76 was the hexagonal startup screen: it had 12,125 cyan
  header and 41,572 colored level-column pixels but zero bright pixels in the newly measured fixed
  label ROI. All 45 retained canonical frames from that run are `unknown` under the revised layout;
  their largest fixed-label count is 814 against the new 4,000 minimum.
- `2026-08-24 14-54-57.mkv` is registered outside the repository under distinct
  Gamescope-vkCapture/OBS capture profile
  `f5f0c5a86b5edba6a8fd014ad85b3873be8f745c0b531d2b5b77f203770b046a` and canonical normalizer
  `75cb7c90e8fc8e430b8f3d2f33f77208971556987bc7d82066a351c3aa4d4e09`. Its 346-frame one-second
  extraction classified exactly five result frames at PTS 291,000 through 295,000 ms. Their warm
  minimum was 3,360 and both panel-edge counts were 522; among non-result frames with a passing warm
  count, the largest minimum of the two edge counts was 35. The reviewed result is `FAILED` for
  `airflow -dreaming of the sky- Game Edition` by `ウッチーズ`, song ID
  `5ce4a9b5-6d3c-575a-8f9e-7646ce8c18b1`.
- The SDL marker artifacts remain valid controlled evidence for their own explicitly bound backend,
  but they are not the live INFINITAS binding and cannot admit a Wayland session.
- A fresh controlled marker session admitted that private binding after observing 2556x1428 BGRx
  MemFd with 10,224-byte stride. A separately restarted session with only declared nested refresh
  changed from 120 to 119 was rejected as `profile_nested_refresh_mismatch`; both runs recorded one
  compact admission fact and completed receiver/provider shutdown with no dropped facts. A
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
  clippy with warnings denied, 161 library tests, 163 binary tests, 55 corpus tests, 75 offline OCR
  tests, dataset E2E, and
  native PipeWire build verification. Focused session tests also verify descriptor/layout rejection,
  frame-generation rejection, diagnostic opt-out non-interference, and manifest-backed ordered
  binding rollover.
- Focused routing tests exercise synthetic result and music-select inputs, title-bearing exact
  screen-local field sets, retained live owner identity, diagnostic opt-out, and structural unknown
  exclusion. Existing offline result, music-select, and integrated-context export tests pass through
  the same routing function.
- Focused field-worker tests verify one pre-worker loader call, worker-thread execution, result
  provenance, rejection across either run ID or immutable binding, non-blocking queue capacity,
  global accepted-but-unconsumed capacity, race-free abandoned-result accounting, supervisor
  retention through blocking observer teardown, destructor-inclusive bounded finish timeout, and
  nonterminal timeout reporting. Replay-bound descriptors retain their replay identity and are
  accepted by the common worker; noncurrent-layout descriptors fail before the loader executes.
- Focused resource-loader tests verify the canonical runtime manifest, pre-I/O model/runtime
  mismatch, location-specific I/O source preservation, absent active catalog, active-catalog digest
  mismatch, and that the resource-owning observer type satisfies the ADR 0030 `Send` boundary.
- On the development machine, the read-only resource gate loaded active catalog
  `ceabe2931815c492b9eb088282ab6df55cabff2545fd9d8de3e0ae11b1b2b541`, registered model
  `5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634`, and runtime
  `4864f57937b6d57510e82234325f611df31521ff508767011de137bebdf531dc` into one CPU session,
  transferred ownership to the production field worker, and completed bounded teardown with no
  submitted crops. Changing only the runtime digest failed before resource I/O with typed
  `runtime_binding_mismatch` and exit status 2.
- Focused integrated-session tests route one synthetic current-run result crop set through a worker,
  retain its complete bound output, and record one compact field fact. Recording opt-out returns
  the same complete output with no artifact. A capacity-one run preserves its second screen result
  while reporting `field_observer_outstanding_limit`, counts the unconsumed first result as
  `field_observation_abandoned` at its exact sequence, and finalizes as partial. Another run cannot
  consume that pending output, and a disconnected pending reports worker loss only once.
- Focused candidate-domain tests retain every synthetic catalog song when title and artist evidence
  conflict, preserve the two music-select title presentations as separate evidence, verify
  collision-safe width-fold comparison including a cross-song collision, fail typed on a
  search-term-only song, verify independent integer absolute/normalized metrics, and represent an
  empty catalog as zero candidates without inventing an unknown-song decision.
- The revised development-machine recording profile remained outside the repository and was
  selected by SHA-256 `dfcc25e8b3f8db9d5a8362a9817112e2b2dbeee14ebbef389edc79fac755ee5b`.
  It binds the corpus recording `2026-08-17 19-25-31.mkv` by digest, its 459-frame canonical
  extraction, the current layout and registered catalog/model/runtime, 250 ms source pacing, a
  5,000 ms diagnostic frame cadence, and three reviewed result windows. The production field path
  inspected all 459 frames, classified exactly 24 result frames inside those windows, completed all
  three episodes as two exact `FAILED` and one exact `CLEAR`, submitted 120 field observations,
  retained 120 full-catalog candidate sets and 305,760 song scores, and observed the expected exact
  clear type on 22 frames. The field worker and diagnostic run both completed; the final 92-frame,
  579-fact diagnostic manifest had no drops or degradations and SHA-256
  `59f406505e9226bf93e0f2ca9c76ac96fa9c2f30b6d1d7a8cd73c8e5f1008387`.
- A development-machine value-bearing replay retained 120 field observations, the exact
  2,548-song catalog string table, and 305,760 candidate records. It established the resolver inputs:
  `ABSOLUTE EVIL`/`Yuta Imai` at title/artist edit zero and title margin four, and `ANEMONE` observed
  as `ANEMON` with title edit one, `6/7` similarity, margin two, and artist similarity `20/43`.
- The create-only profile v2 digest is
  `dfcc25e8b3f8db9d5a8362a9817112e2b2dbeee14ebbef389edc79fac755ee5b`. The final simulation
  inspected 459 canonical frames and completed 3/3 episodes: two `FAILED` results for
  `6ef33da9-090a-500c-844a-8bffd14de63f` (`ABSOLUTE EVIL`) and one `CLEAR` result for
  `5570fd25-7cb9-55b6-8f15-bcbe46de4ad6` (`ANEMONE`). It produced 22 exact song decisions, 22
  exact clear-type matches, two typed `empty_title` transition unknowns, and no wrong acceptance.
  The compact v3 observation encoding retained all 120 observations in 7,147,256 bytes rather than
  repeating the 2,548 song IDs and field names per row. Its complete evidence manifest has SHA-256
  `4a039335fc048e1ea4320e5dd3892a30ff555405a64796aceee7d142dc8e7e54`; its complete 92-frame,
  579-fact diagnostic manifest has SHA-256
  `59f406505e9226bf93e0f2ca9c76ac96fa9c2f30b6d1d7a8cd73c8e5f1008387`, with no drops or
  degradations.
- Replaying the same 459 canonical frames under layout SHA-256
  `2c9b2356be59bf86a48ebaa8878cf01206b9ea6ac18b212d973c861aed7ef6ac` left all 24 result frames
  unchanged and retained 89 structurally anchored music-select frames. Their fixed-label counts
  ranged from 4,660 to 5,962. The production simulation submitted 113 field frames and completed
  all three episodes with 22 exact song decisions and 22 exact clear-type matches; field worker,
  diagnostic manifest `c058a36f7113c0924dc607101937a96f2e4ff484db59ac7755077942ce3df3a4`,
  and recognition artifact `474020b991302fcc03e940cadecbf4dac476f09656d196868bf8e91dfeced99e`
  were complete.
- Exact direct-live frame 314 has QOI SHA-256
  `ac478bc21cdca91caa5e052200bc58406685e593e59c3b7cfb590998c66239bd` and canonical pixel
  SHA-256 `e52c2f9466281e847b9ce46b3ac9da0e6a6bc1e150c072cc6a7b8da849372dbf`.
  Under layout SHA-256 `2c9b2356be59bf86a48ebaa8878cf01206b9ea6ac18b212d973c861aed7ef6ac`,
  it is `result`: warm 3,956, upper edge 521, lower edge 523, with unchanged edge minimum 518.

## Unverified boundaries

- The result-song resolver is grounded by two song identities and three result episodes from one
  recording. It has no title-disjoint holdout, broader clear-type/background coverage, calibrated
  false-accept denominator, or release-accuracy authority. Music-select song resolution, charts,
  digits, temporal result-event emission, and deduplication remain unimplemented.
- Earlier admitted Gamescope runs drove the superseded two-color music-select predicate; a true
  live music-select frame has not yet been observed under the fixed-label predicate. An exact live
  result QOI still classifies as result offline. Recording evidence retains
  replay provenance and cannot fabricate the live generation/profile/normalizer owner. No
  development-machine run has measured live inference-plus-scoring cost, queue behavior, or
  candidate output as a target-host performance gate.
- The bounded CLI gate is not an ordinary long-running application loop. Live queue-full or worker
  loss was not forced on the development machine; bounded unit tests cover queue drop, worker loss,
  generation rejection, opt-out, and diagnostic non-interference. Target-host cost remains unknown.
- The live recognition-artifact worker is covered by exact-value/timing, create-only, unavailable,
  compact-link, clippy, and workspace tests and the earlier complete-cadence path has retained
  Gamescope frames. The new foreground-compacted retention has complete recording simulation and
  unit evidence but not yet a prospective live result.
- Session provenance is explicit launcher/operator input, not an automatic observation of the
  Gamescope process. Process discovery or attestation is not implemented or claimed.
- The new identity-geometry 1920x1080 Wayland binding has controlled-marker evidence only. A
  user-started INFINITAS session has not yet admitted it or demonstrated result recognition under
  that binding.
- INFINITAS content/geometry, target play-machine output, 4K, FSR/NIS, Reshade, HDR, Portal, and OBS
  are separate uncalibrated domains. The development-machine profile is not a pixel reference.
- OBS/obs-vkcapture coexistence, PipeWire daemon disconnect, stream loss distinct from node loss,
  source recreation, long soak, FD/thread/RSS convergence, frame age, CPU/memory/copy/GPU/power
  cost, game p99 frametime, and OBS lag remain unverified.
- Offline music-select field submission and replay of the exact retained live result predicate are
  verified, but prospective fixed-label live music-select, direct-live result OCR/resolution, graceful
  stdin-requested shutdown, queue/artifact completion, accepted field gate, event daemon,
  target-host performance gate, and supported capture profile remain unverified. The corrected
  edge coordinates did not lower the predicate threshold. The earlier run has no paired raw source,
  so its exact source-to-canonical transform cannot be reconstructed; new foreground runs can
  retain that pair, but the exact offline transform comparison command is not yet implemented.
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

1. Add a bounded offline transform inspector that selects one manifest-bound raw BGRx/canonical QOI
   pair and an expected Gamescope binding digest, reruns the registered fractional normalizer, and
   reports exact/aggregate pixel differences without another capture session.
2. Extend prospective retention evidence only if the bounded offline transform inspector exposes a
   missing source contract, export, or capacity invariant; do not broaden routine raw retention.
3. At the next naturally occurring user-started INFINITAS session, select the new identity-geometry
   1920x1080 Wayland binding and require exact admission plus live result title, artist, `CLEAR TYPE`,
   song decision, complete diagnostic/recognition artifacts, and ordered scorepeek teardown before
   defining event authority, target-host performance acceptance, or support. Do not request another
   play merely to tune the repaired predicate.

Do not proceed to automatic calibration, Portal/OBS fallback, event emission, soak/performance, or
support claims until transform replay and a prospective normal live recognition session have
complete immutable evidence.

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
