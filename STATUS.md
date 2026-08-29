# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it.
Uncommitted working-tree state is outside this checkpoint. Implementation history belongs in Git;
the roadmap and long-lived decisions remain in `docs/plan.ja.md` and `docs/decisions/`.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 offline canonical-frame and recognition spike: **in progress**.
- Current execution focus: the corpus recording has passed the value-bearing result-song
  recognition simulation for all three reviewed episodes, and a normal foreground Gamescope
  session now reuses that post-canonical path. Retained Wayland evidence repaired three independently
  measurable layout errors without lowering thresholds or changing the OCR model: the two result
  panel edges were one row low, the result title region contained excess blank height, and the
  result artist region retained only its center. The revised text regions reproduce all three
  operator-confirmed live title/artist pairs from retained QOIs and pass the complete three-episode
  recording simulation. Foreground evidence is bounded for hours-long use and selected result
  frames can retain paired exact raw BGRx for later transform replay. The layout also requires the
  measured fixed `MUSIC SELECT` label and rejects retained startup evidence offline. Retained
  longest-title live evidence now grounds a screen-local music-select resolver: the clipped
  one-line active row is primary prefix evidence, while the arbitrary central-title texture and
  artist can only strongly corroborate or narrow a tie. Complete recording and active-catalog
  replays accept the reviewed selections without weighted score fusion or threshold relaxation.
  No independent transform mismatch or changed normalizer currently justifies replaying a
  scorepeek-written raw/canonical pair through the same implementation. ADR 0048 trusts
  operator-selected local artifacts, removes that transform-first checkpoint and duplicate
  problem-report retention. ADR 0049 replaces the custom private deployment unit with a standard
  cargo-dist Linux x86-64 archive and checksum while keeping the catalog outside the archive.
  ADR 0050 classifies the fixed Apache-2.0 PP-OCRv6-small model as a disposable XDG cache: every
  non-information CLI invocation ensures it globally, while help, version and doctor stay offline.
  ADR 0051 adds the ordinary capture-profile surface: guided setup launches a dedicated
  scorepeek-owned Gamescope marker, publishes one create-only machine-local canonical binding,
  lists local profiles, and lets `scorepeek run` select a profile by name while reusing the existing
  foreground diagnostic and provisional-recognition path. Real target calibration remains
  unverified. ADR 0052 makes that ordinary entrypoint a Gamescope-non-owning watcher: it waits
  before or after source startup, treats sequential source lifetimes as separate sessions, refuses
  simultaneous-source selection, and stops only on SIGINT/SIGTERM.
  ADR 0053 removes blanket symlink rejection from operator-selected local roots and inputs while
  retaining resolved content validation, create-only no-clobber publication, and non-following
  owned cleanup. This includes Bazzite's standard `/home -> /var/home` layout.
  ADR 0054 replaces guided setup's aspect-fit assumption and complete-pixel marker threshold with
  a measured positive axis-aligned X/Y transform. The machine-local v3 profile stores only actual
  BGRx dimensions, the measured rational source rectangle, canonical contract, and normalizer
  identity; ordinary admission no longer compares launch metadata or Gamescope version.
  ADR 0055 corrects crop admission to use the canonical pixel-center sampling footprint, allowing
  signed half-pixel scaler phase while still rejecting missing required samples.
  ADR 0056 fixes application recognition at 10 Hz with latest-frame/no-backlog live sampling and
  deterministic source-time video sampling. A verified v3 diagnostic containing session NDJSON
  streams and bounded deduplicated QOI evidence is now the only capture-regression corpus input;
  operator review, immutable labels, suite publication, and production frame replay are separate
  stages. Video is auxiliary and the former recording-dataset CLI routes have been removed. The
  active frame-first suite generation
  `133d408c074951a6f150e4da529a48a68c1f66e05250d78c2e6c55adae8fad9f` contains four verified
  diagnostics and fourteen operator-reviewed episodes. `mise run corpus:test` replays all 1,870
  stored canonical frame references through the production predicate and all fourteen stable QOIs
  through OCR, catalog resolution, and clear-type resolution successfully. The current
  suite also contains one operator-confirmed black startup frame as an explicit negative predicate
  expectation. The current four-song legacy diagnostic was normalized to v3 with 636
  canonical QOIs and 32,768 facts in one NDJSON stream; the video diagnostic deterministically
  processed 4,584 10 Hz observations and retained 272 deduplicated canonical QOIs.
  ADR 0057 separates retryable pre-admission failure from an admitted session's terminal failure.
  A target scorepeek-first run observed one startup admission rejection and then remained stuck on
  the consumed numeric node; restarting scorepeek after Gamescope stabilized admitted immediately.
  The watcher now consumes a node only after session start and retries a unique not-yet-ready source
  every 500 ms without repeated output. The same target run retained two visually reviewed
  `EXH-CLEAR` results (`The Commanders` and `Forgetting Machine`) as a verified partial v3 diagnostic
  with 324 canonical QOIs. Its independently bounded fact stream omitted predicates retained by the
  recognition stream, so v3 publication now uses each self-contained recognition timestamp and
  scene when that partial join input is absent.
  ADR 0058 separates the provisional run observation transport from terminal presentation.
  `$XDG_RUNTIME_DIR/scorepeek/observations-v2.sock` now sends a current snapshot followed by
  sequenced `scorepeek-run-event-v2` records through a bounded non-blocking multi-client worker.
  TTY stdout renders watcher state, separate OCR and catalog-backed song presentation, resolver
  evidence, and channel health without raw mode; non-TTY stdout reports only deduplicated human
  state changes. This does not implement the accepted `/v1.sock` event API or event authority.
  ADR 0059 adds a deterministic result-local temporal reducer after the frame resolver. Song and
  clear type stabilize independently after two equal observations within 250 ms; stable values
  survive a transient unknown, while a different accepted value becomes a typed conflict. Raw
  field observations remain unchanged and precede bounded `temporal_result_changed` transition
  records; synchronous `screen_changed` records reset the reducer across non-result boundaries.
  Ten temporally analyzable reviewed episodes contain 430 correct song accepts, ten
  unknowns, no wrong accepts, 420 correct clear values, twenty unknowns, and no wrong clear values;
  four sparse legacy episodes are excluded from temporal calibration. The read-only
  `scorepeek-private-temporal-evaluation-v1` evaluator now verifies the active suite and observation
  object bindings and runs the production reducer over those ordered intervals. Both 2/250 ms and
  3/250 ms finish jointly stable-correct on all ten analyzable episodes with no wrong stability,
  conflicts, gap resets, or pending replacements. The 2-observation policy reaches joint stability
  at p50 591 ms / p95 796 ms versus p50 782 ms / p95 1,003 ms for 3 observations, so the retained
  evidence does not justify adding the third observation. Three sparse episodes expose one retained
  result observation and one has no bindable result interval. Music-select dwell remained
  unimplemented pending operator-reviewed stationary and scrolling spans; ADR 0063 now completes
  that review evidence, while candidate policy evaluation and policy selection remain pending.
  ADR 0060 adds the preceding music-select motion-review surface without changing that boundary.
  The create-only offline command verifies one active-suite 10 Hz FFV1 video-replay session and
  measures adjacent frame movement separately for the twenty-row right-list union, active-list
  title, and central title while retaining 500 ms of screen-transition context. Its first complete
  run over the bound ordinary-session video produced eleven unlabeled spans, 982 samples, and 971
  adjacent pairs in a 442 KiB draft; every span remains typed `unknown/operator_review_required`.
  That run independently matched every decoded-frame PTS against selected packet PTS and retained
  observation timestamps after verifying the session's full video, profile, normalizer, and layout
  binding. A release-review follow-up fixed the video to one open identity with a pre-publication
  rehash, stops each decoder after its selected output count, retains ROI pixels only for the current
  span, and supervises every probe/decode child with a wall-clock deadline and reap path. The same
  982-sample draft was reproduced byte-for-byte from the real recording after those fixes, and its
  worker exited successfully without a residual process.
  Visual inspection confirmed why whole-frame motion is unsuitable: a stable 105.0--105.1 s
  selection retained the same list and title while central animation continued, whereas the
  103.4--103.5 s high-list-motion pair crossed from the difficulty category into actual titles.
  No observed metric has been promoted to a label or threshold.
  ADR 0061 adds a create-only digest-bound review-application contract at adjacent-pair granularity;
  one label per original span was rejected because the longest span is 23.4 seconds and mixes
  behavior. Of the 971 pairs, 838 whose two screens are music-select are eligible for operator
  decisions and 133 remain typed screen context. Bounded inclusive sequence intervals expand to
  exact pair identities, overlap or context crossing fails closed, and omitted eligible pairs stay
  unknown. The initial empty-decision application reproduced those counts with `complete=false`;
  it added no operator decision or dwell truth.
  ADR 0062 corrects the review contract after the bound video exposed a production-predicate false
  positive: sequences 898--907 visibly remain on MODE SELECT even though all retained predicates
  say music-select. At sequence 898 the recorded header, level-column, and label counts were
  8,740/7,000, 26,743/1,000, and 4,892/4,000 respectively, while the draft's packet, decoder, and
  observation timestamps already agree. Review-decision and reviewed-set v2 therefore let the
  operator exclude a predicate-eligible pair as typed screen context without inventing motion.
  Applying the visually reviewed sequence 899--907 interval produced nine operator-context pairs,
  133 predicate-context pairs, and 829 pairs still requiring review with `complete=false`. The
  production predicate, motion thresholds, dwell, and event authority remain unchanged.
  ADR 0063 fixes deterministic authoring precedence for the remaining visual review: visible active
  selection identity changes take precedence over concurrent list motion, same-selection row
  translation or settling is scrolling, and non-list animation is ignored. The complete
  digest-bound application now covers all 971 adjacent pairs: 712 stationary, 84 scrolling, 30
  selection-change, 12 operator-context, and 133 predicate-context pairs, with no remaining review
  pair and `complete=true`. The reviewed-set digest is
  `aa59dc31a678c4db633db0391747642de49a48e466bf53421c2054f9c68b912e`, bound to draft
  `f7d205cb38f9f29848f7b11261da0e0dee491fa172189d27997ce6cc68b36b5e`. This establishes motion
  review truth only. ADR 0064 adds a create-only offline dwell evaluator which verifies the bound
  active suite, session observation object, and exact content-addressed catalog generation, then
  replays the retained OCR strings through the production music-select resolver. The default
  100/200/300/500 ms equal-accepted-ID candidates cover 16/27, 16/27, 13/27, and 13/27 stationary
  runs, retain 24/18/17/16 nonstationary stable pairs, and each miss two selection-change resets.
  The 500 ms candidate loses coverage without eliminating false stability, so no time-only runtime
  policy, stable-selection accuracy, runtime state, or event authority has been selected. The
  canonical evaluation SHA-256 is
  `5c7954152b95ed6f14b58b7992643df62ef0879841997680fa59cb24318c8a8c`.
  The archive and active catalog have been transferred to the first operator-owned 4K Bazzite
  machine, where the installed CLI passed `--version` and `doctor` and fetched the registered small
  model. After the `/home` symlink fix, retained synthetic target evidence showed the 1920x1080
  marker in a 3840x2160 BGRx frame with intact fiducial interiors and only bounded scaling-filter
  boundary differences. The first measured-transform run authored a v3 profile with rectangle
  `(0, 0, 3839.5, 2159.5)`. A repeat exposed a valid half-pixel scaler phase that the continuous
  rectangle check misclassified as crop. After ADR 0055 aligned admission with the normalizer's
  sampling footprint, the same target command authored `gamescope-4k` from nine fiducials with
  rectangle `(-0.5, -0.5, 3840, 2160)` in the observed 3840x2160 frame. This proves calibration and
  production normalization for the marker. Two ordinary watcher runs have retained six reviewed
  result episodes, but the fixed pre-admission retry, automatic partial v3 publication, and the
  remaining lifetime/signal matrix require a fresh target run.
  Release accuracy, event authority, target-host performance, and support remain later gates.

## Included deliverables

### Catalog and private corpus

- Strict Tachi, Textage, and dqn acquisition, parsing, provenance, deterministic federation,
  quarantine, last-known-good activation, bounded private caches, and durable catalog snapshots.
- `scorepeek catalog sync` plus manual, persistent user-systemd, and transient scheduling routes.
  The CLI retains actionable credential-free adapter, transport, cache, and store error causes.
- Separate offline-only `scorepeek-corpus` tooling verifies and imports v3 diagnostics, applies
  immutable operator labels, atomically activates suite generations, and replays every active
  frame. Historical recording-store internals remain testable for OCR work but their direct
  capture-regression CLI routes are removed. Runtime code has no corpus or Python training
  dependency.
- Private frames, labels, recordings, source snapshots, generated catalogs, models, and environment
  artifacts remain outside the repository. Committed fixtures are synthetic or opaque and
  non-personal.

### Canonical recognition and diagnostics

- Fixed contiguous RGB8 1920x1080 canonical-frame contract with one shared layout, fail-closed
  result/music-select crops, contextual title recognition, and selection-song context. Music-select
  presence requires the fixed label structure in addition to the existing header and level-column
  palette evidence. Result title and artist use measured text-tight regions; dependent context
  layout bytes bind the same canonical layout digest.
- PP-OCRv6 small native-dynamic is the selected title observer. Registered model bundles,
  preprocessing, CTC decoding, exact-first comparison keys, catalog search, and private replay
  tooling are digest-bound and reproducible. Custom training/export is deferred until integrated
  evidence isolates a residual that requires it.
- Application-owned v3 diagnostic sessions retain full 10 Hz fact and observation NDJSON streams
  plus bounded content-deduplicated canonical QOI evidence. Live sampling uses only the latest
  frame and counts busy ticks without backlog; video sampling uses source timestamps. Optional
  observed QOI pairs preserve normalization evidence without storing uncompressed BGRx in new
  diagnostics. A session records at most 250,000 fact or observation records, and each NDJSON
  record is bounded to 1 MiB. Pixels and recognition facts remain separate from public result
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
- Developer create-only calibration sampling records exact bounded environment, Gamescope version, backend,
  output size, nested size/refresh, scaler/filter, complete observed BGRx contract, frame digest,
  receiver sequence, monotonic receive time, and typed capture facts. Hashing, serialization,
  filesystem publication, and fsync occur after receiver/provider shutdown.
- Machine-local `scorepeek-gamescope-profile-binding-v3` is canonical JSON selected by an
  independent SHA-256. It binds default Gamescope source kind, observed BGRx width/height, the
  measured 1/2048-pixel rational source rectangle, fixed canonical contract, and normalizer
  implementation. It contains no launch arguments, Gamescope version/backend/scaler/filter,
  refresh/color metadata, calibration stride/memory type, or discarded frame digest.
- Setup fits nine redundant marker fiducials to independent positive X/Y scale and translation,
  rejects residual above one observed pixel and a canonical pixel-center sampling footprint outside
  the frame, then
  verifies fiducial and cell interiors through the production normalizer. Correctable padding,
  offset, fractional phase, non-integer/anisotropic scale, aspect distortion and bounded filter
  edges are accepted; crop and non-axis-aligned transforms fail closed.
- ADR 0055 aligns crop admission with the production normalizer's half-pixel convention. Source
  left/top rationals may be negative; every first/last canonical pixel-center sample must remain in
  observed pixel support, while width/height stay positive. The target-observed
  `(-0.5, -0.5, 3840, 2160)` phase is therefore correctable rather than cropped.
- Runtime admission requires actual BGRx dimensions to match the profile and the receiver to have a
  valid current memory/stride/byte layout with in-bounds saved geometry. Launch provenance,
  Gamescope version, filter, scaler, refresh/color metadata, and calibration-time allocation do not
  participate. A rejected receiver remains explicitly shut down by its owner.
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
- ADR 0046 adds the fail-closed music-select song resolver
  `scorepeek-music-select-active-prefix-corroborated-v1`. It treats the clipped one-line active-list
  title as primary catalog-prefix evidence, requiring at least five folded comparison-key units,
  edit distance at most one, and similarity at least `6/7`. Central-title texture and artist observations remain
  separate one-crop OCR evidence; only full-text matches within edit one and similarity `4/5` are
  strong enough to conflict with a unique active candidate or intersect an active tie. Weak
  supplemental OCR is ignored, and every empty, short, weak, conflicting, or ambiguous result is a
  typed unknown. No weighted score is computed.
- Recording profile v2 requires an exact expected `ScorepeekSongId` for every episode. The
  recognition simulation requires at least two exact expected song decisions and two exact
  expected `CLEAR TYPE` observations per episode, rejects a different accepted song immediately,
  and retains sequence/PTS, exact OCR, exact catalog strings, candidate counts plus the resolver's
  selected/runner-up evidence,
  decisions/reasons, and expected values in a create-only bounded local artifact. Catalog JSON is
  capped at 16 MiB; observation NDJSON is capped at 512 MiB, 1 MiB per record, and 250,000 records;
  the manifest is
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
- ADR 0040 adds the developer `scorepeek run gamescope` gate. ADR 0052 supersedes its ordinary
  single-session and stdin-control lifecycle with a signal-stopped multi-session watcher. ADR 0058
  moves its exact bounded field/resolver NDJSON from stdout to the provisional observation socket,
  preflights an enabled private diagnostic root, records full numeric screen-predicate evidence for
  unknown as well as recognized screens, and finalizes the existing field, diagnostic, and
  recognition-artifact workers in order. Its control path does not signal Gamescope, INFINITAS, or
  the process group.
- ADR 0043 made the former compacted foreground artifact practical for multi-hour sessions.
  ADR 0056 replaces that ordinary-run retention with the complete bounded v3 stream. The historical
  policy retains one representative result per interval, splits result observations separated by more
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
- An operator-started 1920x1080 Wayland INFINITAS session admitted the identity-geometry binding as
  capture generation 30 and shut down through the foreground stdin contract with exit status zero.
  It normalized and inspected 1,287 frames, including 68 result and 191 structurally anchored
  music-select frames. The recognition artifact retained 59 bounded result observations across the
  three songs confirmed by the operator: `LIGHTNING STRIKES`, `Voo Doo Bamboleo`, and
  `quick master (reform version)`. Its diagnostic recording was partial because 53 observations hit
  the field-observer outstanding limit and 46 offers found the queue full; this is live
  backpressure evidence, not a complete recognition gate.
- Exact retained result QOIs at sequences 383, 561, and 1,187 reproduced the text-region failure
  offline. With only the committed title region changed from `660,900,600,100` to
  `660,950,600,50`, the registered dynamic OCR input widened from 320 to 576 and decoded all three
  confirmed titles exactly. With only the artist region changed from `850,990,220,35` to
  `650,990,650,40`, its input widened from 320 to 780 and decoded `BEMANI Sound Team "HuΣeR"`,
  `SOUND HOLIC Vs. ZYTOKINE feat. CALEN`, and `youhei shimizu` exactly. Paired raw and canonical
  evidence at sequence 1,187 decodes to byte-identical RGB24, so that retained sample does not
  attribute the text failure to the normalizer.
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
  clippy with warnings denied, 297 library tests, 215 binary tests, 69 corpus library tests, 3 corpus
  binary tests, 77 offline OCR tests, and native PipeWire build verification. Focused session tests also verify descriptor/layout rejection,
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
- Focused run-output tests verify snapshot-before-live socket delivery, owned-socket cleanup,
  stale-socket replacement without overwriting a non-socket entry, multiple/no/slow-client behavior,
  queue-drop accounting, typed state reduction, separate OCR/catalog rendering, narrow-terminal
  title/artist priority and ellipsis, and accepted song presentation with resolver evidence.
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
- Replaying all 459 canonical frames under result-text layout SHA-256
  `316113f34b3844e2b53d010e1c529c70a9ba032d2d950b051ae5b302937119a5` again classified 24 result
  frames and submitted 113 field frames. All three episodes completed with 22 exact song decisions
  and 22 exact `CLEAR TYPE` matches. The field worker, diagnostic manifest
  `46095e7c3419fb3a2b82d5a43a31333bdc56bc28d6c1df2f4c273e362400dc19`, and recognition artifact
  `ca1ccebc69bb897cf23b47466aa4e1eeb95c8d1a2948b20ec1ad7d6aefa7c480` were complete. The accepted
  `ANEMONE` observations decoded its 36-unit artist exactly; `ABSOLUTE EVIL` remained accepted with
  title edit zero and artist edit one.
- Replaying all 459 canonical frames under canonical layout SHA-256
  `6b56454a3023d6d3900682396b77f41e8919cb95c1444c83be08c48cb1dacfa4` and recording profile
  SHA-256 `fac16cc2a6c6ad2790ababb8c8a3d7ae990d8464156343b2642f9544d8424e11` completed all three
  episodes. It submitted 113 field observations, retained 287,924 full-catalog candidate records,
  and preserved the existing 22 exact result song and `CLEAR TYPE` matches. Music-select produced
  72 accepts covering the four songs actually visible during selection and scrolling; 16 blank,
  menu, difficulty, or garbled observations remained typed unknown. The complete 60-second-cadence
  diagnostic manifest is `4d1c624aa0af5b75f8a0980e469d7b442d7c7240555a5ba2323aa018804ed90d`
  and the complete recognition artifact manifest is
  `409a9dc074fd418866c5cc3d51d8b4252aa75eb5109b1e1545cd1a3844139780`.
- The same resolver and active 2,548-song catalog accept retained direct-live active rows
  `MOVE! (We Keep It Movin')` and clipped `ASIAN VIRTUAL REALITIES (MELTING TOGETH` at prefix edit
  distance zero with runner-up edit margins 15 and 25. Their imperfect central-title texture and
  artist observations are below the strong-evidence threshold and do not participate.
- Exact direct-live frame 314 has QOI SHA-256
  `ac478bc21cdca91caa5e052200bc58406685e593e59c3b7cfb590998c66239bd` and canonical pixel
  SHA-256 `e52c2f9466281e847b9ce46b3ac9da0e6a6bc1e150c072cc6a7b8da849372dbf`.
  Under layout SHA-256 `2c9b2356be59bf86a48ebaa8878cf01206b9ea6ac18b212d973c861aed7ef6ac`,
  it is `result`: warm 3,956, upper edge 521, lower edge 523, with unchanged edge minimum 518.

## Unverified boundaries

- The result temporal reducer is grounded by ten reviewed episodes with complete enough ordered
  observations; four legacy episodes are too sparse for temporal calibration. The offline policy
  comparison confirms retained-corpus coverage and latency only; it has no wrong-accept challenge
  set, title-disjoint holdout, calibrated false-accept denominator, or release-accuracy authority.
  The screen-local music-select resolver is grounded by recording-visible selections and
  separate long-title live observations, but stable-selection dwell, charts, digits, accepted
  event emission, and deduplication remain unimplemented. The complete motion review is sufficient
  for candidate dwell evaluation but does not select a policy.
- A true live music-select screen has passed the fixed-label predicate and retained values now pass
  the screen-local song resolver, but stable-selection and event acceptance remain unimplemented.
  Exact live result QOIs classify as result and reproduce their
  three confirmed title/artist strings under the revised regions offline. Recording evidence retains
  replay provenance and cannot fabricate the live generation/profile/normalizer owner. The partial
  live run exposed queue pressure and candidate output, but no development-machine observation is a
  target-host inference-plus-scoring performance gate.
- The bounded CLI gate is not an ordinary long-running application loop. Live queue-full or worker
  loss was not forced on the development machine; bounded unit tests cover queue drop, worker loss,
  generation rejection, opt-out, and diagnostic non-interference. Target-host cost remains unknown.
- Ratatui rendering and the observation socket are covered by deterministic development-host tests,
  but an installed target archive has not yet exercised alternate-screen restoration, redirected
  plain output, mid-session socket attachment, slow-client disconnection, or a real long-title
  observation. These checks grant neither event authority nor target capture support.
- The live recognition-artifact worker is covered by exact-value/timing, create-only, unavailable,
  compact-link, clippy, and workspace tests and the earlier complete-cadence path has retained
  Gamescope frames. The foreground-compacted retention has retained three live results and complete
  recording simulation, but its first three-result live run was partial under field-observer
  backpressure. A later longest-title run completed its foreground recognition artifact under the
  revised result text regions, but predated the current music-select crop and resolver.
- Historical developer samples may retain launcher metadata, but ordinary source lifetimes and
  profile admission do not accept or compare operator-declared session provenance. Process
  discovery or attestation is not implemented or claimed.
- The identity-geometry 1920x1080 Wayland binding has controlled-marker evidence and admitted
  user-started INFINITAS foreground sessions. The first three-result run was partial and used the
  superseded text regions; later runs do not yet constitute a supported-profile gate.
- INFINITAS content/geometry, target play-machine output, 4K, FSR/NIS, Reshade, HDR, Portal, and OBS
  are separate uncalibrated domains. The development-machine profile is not a pixel reference.
- OBS/obs-vkcapture coexistence, PipeWire daemon disconnect, stream loss distinct from node loss,
  source recreation, long soak, FD/thread/RSS convergence, frame age, CPU/memory/copy/GPU/power
  cost, game p99 frametime, and OBS lag remain unverified.
- Offline current-layout music-select submission, prospective fixed-label live music-select, three
  direct-live result observations, a later complete recognition artifact, and graceful
  stdin-requested shutdown are verified. Retained longest-title values resolve under the current
  code, but the current music-select crop and resolver have not yet run prospectively from live
  pixels. Stable-selection acceptance, event daemon,
  target-host performance gate, and supported capture profile remain unverified. The corrected
  edge coordinates did not lower the predicate threshold. The earlier run has no paired raw source,
  so its exact source-to-canonical transform cannot be reconstructed. New foreground runs can
  retain a pair if a future observed transform mismatch or normalizer change requires comparison;
  the absence of a transform inspector is not a current execution blocker.
- Current recordings and provisional labels do not establish title-disjoint result accuracy,
  calibrated thresholds, result dwell, miss denominator, deduplication, or release accuracy.
- The complete music-select motion review and ADR 0064 evaluation are descriptive evidence only. Of the 838
  predicate-eligible adjacent pairs, 712 are stationary, 84 are scrolling, 30 are selection
  changes, and 12 are operator-confirmed non-music-select context; the separate 133
  predicate-context pairs remain unknown context. Replaying equal accepted song IDs shows that
  100--500 ms time-only dwell cannot clear two observed selection changes. The motion truth does
  not label the correct song ID, so it establishes neither stable-selection accuracy nor a usable
  reset classifier; the observed predicate false positives have not been recalibrated.
- Persistent scheduler installation was verified in isolation but not applied to the operator's
  actual configuration. Real S3 credentials/provider behavior and remote bucket lifecycle remain
  untested.
- Offline corpus consumers now follow ADR 0048's trust boundary. Probe and extraction hash a selected
  source once; sealing and replay-index generation do not rehash source media; remote push omits a
  redundant complete local preflight and post-upload reread; pull hashes downloaded remote bytes and
  then validates typed bindings. Content-store publication, remote staging/reuse, explicit verify,
  external code/model acquisition, concurrent-writer checks, and activation contracts retain their
  complete verification.
- Music-list measurement reads only the required row crops once. Review planning consumes the
  selected motion artifact and referenced crop manifests without rereading pixels, and review apply
  checks exact artifact/plan/occurrence bindings without reconstructing the plan. Their summaries use
  `source_artifact_bound` instead of claiming `evidence_verified`; explicit motion and observation
  verification remain opt-in complete audits.
- OCR training stages no longer validate every preparation file before reading their own inputs.
  Each stage checks the selected preparation manifest and the files it actually consumes; external
  PaddleOCR checkout and model bundle verification remains at the external-code/resource boundary.
- Cargo-dist 0.32.0 now plans and builds only the `scorepeek` Linux x86-64 CLI archive with its
  standard SHA-256 checksum. The local artifact test verifies the checksum, permits only the binary
  and cargo-dist's README inclusion, and runs `--version` plus `doctor` with isolated home and XDG
  roots. Private resources remain outside the archive and no tag, installer, CI or public release
  path is configured.
- PP-OCRv6-small is not operator data or a release member. The CLI fetches the three files from the
  registered immutable official revision into `$XDG_CACHE_HOME/scorepeek/models`, with exact
  size/digest checks, a writer lock, durable atomic publication and the existing 8-generation,
  192-MiB-object and 512-MiB-total limits. A completed cache avoids network; the fixed loader still
  verifies bytes when used. Catalogs remain separately managed under XDG data.
- `scorepeek setup gamescope --profile NAME -- GAMESCOPE_ARGS...` authors minimal binding v3 from
  measured marker correspondences and the production fractional normalizer. Gamescope arguments
  launch only the dedicated calibration process and are not saved. `scorepeek profile list` reports
  only profile identity, observed dimensions, and measured rectangle; profile-selected `scorepeek
  run` removes binding paths, digests, launch metadata, and repeated provenance from ordinary
  operation. Old local schemas require setup recreation. Existing raw calibration sampling remains
  a developer evidence surface, not runtime identity.
  Each admitted Gamescope lifetime uses a distinct create-only recognition artifact directory;
  `--no-recording` disables watcher status, diagnostic, and recognition artifact persistence.
  Diagnostic resource provenance
  uses the executing binary inode's SHA-256 without changing the CLI version or archive identity.
  One ordinary-run lock serializes admission to the XDG recognition store. At eight generations or
  when the next maximum reservation would exceed 1 GiB, the affected session continues recognition
  without an artifact; scorepeek never deletes an existing run automatically. A single atomically
  replaced watcher record retains only current state, invocation/session links and the last 32
  low-cardinality transitions.

## Approval and authority boundaries

- New dependencies require prior approval with purpose, version/license, alternatives, and
  runtime/distribution/host/reproducibility impact.
- Captured frames, game/player data, complete labels, raw catalog inputs, generated catalogs, OCR
  models, credentials, and environment-specific artifacts must not be committed.
- External-source use remains governed by `docs/sources.md`; no source requiring new permission may
  be enabled without it.
- Push, release, deployment, persistent host configuration, real remote-storage changes, Portal/OBS
  setup, and target-machine changes require explicit user authority.

## Next executable task

1. Exercise the fixed measured target `gamescope-4k` watcher with scorepeek-first startup through the
   transient INFINITAS source sequence, then cover Gamescope-first, sequential and simultaneous
   source lifetimes, idle/active signals, TTY restoration, redirected plain output, and a
   mid-session observation-socket client. Confirm that raw result events precede temporal
   transitions and that one-tick unknowns do not erase the stable TUI result. Do not claim target
   lifecycle or presentation support before those separate checks pass.
2. Verify that the next partial or complete ordinary session publishes v3 automatically without
   component reconstruction, and add only operator-reviewed episodes to the active suite.
3. Verify subsequent offline model-cache reuse without a repository checkout, mise, Rust, or Python
   in the game-session path.
4. Evaluate a bounded runtime-observable music-select reset signal against the complete reviewed
   set. Start with the retained active-row and right-list motion evidence, keep operator truth out
   of runtime input, and report missed selection-change resets, false resets during stationary
   runs, nonstationary stability, and coverage before selecting or implementing any policy.

Do not add transform replay without an independent oracle, broaden routine raw retention, silently
calibrate or switch profiles, request play solely to
tune recognition, or claim event authority, target-host performance acceptance, public
redistribution, or support before the corresponding ADR 0049 delivery checkpoint has complete
evidence.

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
