# Architecture decision index

This index resolves the current decision set without rewriting accepted ADRs.
When an older ADR conflicts with a superseding decision, the newer ADR is
authoritative.

## Current

- [ADR 0004: Treat the Windows implementation as research only](0004-upstream-is-research-only.md)
  supersedes ADR 0001.
- [ADR 0005: Federate external IIDX catalogs without fuzzy identity merging](0005-federate-external-catalogs.md)
- [ADR 0006: Train sequence OCR offline and run catalog-constrained inference in Rust](0006-train-sequence-ocr-run-rust.md)
  supersedes ADR 0003.
- [ADR 0009: Own game layout in the canonical frame contract](0009-own-layout-in-the-canonical-frame-contract.md)
  supersedes ADR 0008.
- [ADR 0010: Preserve recordings as reusable dataset roots](0010-preserve-recordings-as-reusable-dataset-roots.md)
- [ADR 0011: Index FFV1 recordings by packet order](0011-index-ffv1-recordings-by-packet-order.md)
  supersedes ADR 0010's decoded-frame probing method.
- [ADR 0012: Allow path-backed private source objects](0012-allow-path-backed-private-source-objects.md)
  supersedes ADR 0010's mandatory local source copy. ADR 0048 supersedes its unconditional
  post-consumption full local rehash.
- [ADR 0013: Bootstrap the shared layout from a normalized profile](0013-bootstrap-layout-from-a-normalized-profile.md)
  supersedes ADR 0009's multi-profile-first sequencing requirement.
- [ADR 0014: Delegate local filesystem permissions to the operator](0014-delegate-local-filesystem-permissions.md)
  supersedes ADR 0010 and ADR 0012 only for local filesystem mode policy.
- [ADR 0015: Use provisional private title data during development](0015-use-provisional-private-title-data-during-development.md)
  supersedes ADR 0006 only for private-development training-data source policy.
- [ADR 0016: Use stationary music-list rows as result-title evidence](0016-use-stationary-list-rows-as-result-title-evidence.md)
- [ADR 0017: Separate music-list title presentation domains](0017-separate-music-list-title-presentation-domains.md)
- [ADR 0018: Stage title training on stationary music-list evidence](0018-stage-title-training-on-stationary-music-list-evidence.md)
- [ADR 0019: Apply comparison keys to catalog-constrained CTC candidates](0019-apply-comparison-keys-to-ctc-candidates.md)
  supersedes ADR 0006 only for exact-only catalog candidate sequences.
- [ADR 0020: Select an official ONNX recognizer before custom training](0020-select-official-onnx-before-custom-training.md)
  supersedes ADR 0006 for its mandatory fine-tuning/custom-export sequence and ADR 0018 only for
  model candidates requiring set-inclusion growth.
- [ADR 0021: Search the full song catalog from imperfect text observations](0021-search-the-full-song-catalog-from-imperfect-text.md)
  supersedes ADR 0020 only for its direct-encodability or derived-signature evaluation gate.
- [ADR 0022: Select PP-OCRv6 small for contextual song recognition](0022-select-pp-ocrv6-small-for-contextual-recognition.md)
  supersedes ADR 0006's mandatory custom/single-title sequence, ADR 0020's exhaustive phase-two/no-selection requirement, and ADR 0021 only for
  requiring every decoder policy to be compared across every model.
- [ADR 0024: Limit temporal state to selection song context](0024-limit-state-to-selection-song-context.md)
  supersedes ADR 0023's `play_attempt` and full-session state inference while retaining its
  screen-context and recognition-independent recording rationale, and supersedes ADR 0022 only for
  naming play-attempt transitions as the contextual integration gate.
- [ADR 0025: Record bounded application-owned live diagnostic runs](0025-record-bounded-live-diagnostic-runs.md)
  fixes the diagnostic run, storage, completeness, retention, privacy, and non-interference contract
  that ADR 0023 deferred while keeping ADR 0024's minimal recognition state.
- [ADR 0026: Isolate diagnostic I/O behind a bounded application worker](0026-isolate-diagnostic-io-behind-a-bounded-worker.md)
  fixes queue ownership, producer-side cadence, non-blocking live offers, bounded flush, and strict
  canonical replay.
- [ADR 0027: Acquire PipeWire sources behind a common receiver](0027-acquire-pipewire-sources-behind-a-common-receiver.md)
  fixes the source-provider/receiver boundary, selects Gamescope as the first direct PipeWire spike,
  defers Portal to a later provider without automatic fallback, and supersedes ADR 0009 and ADR 0013
  only for treating a future OBS path as an eligible scorepeek capture profile. ADR 0013's existing
  offline OBS/vkcapture recording profile remains valid.
- [ADR 0028: Build PipeWire against a mise-pinned SDK](0028-build-pipewire-against-a-mise-pinned-sdk.md)
  fixes the Linux x86-64 host-native Cargo boundary: mise provides the checksum-pinned PipeWire SDK
  and native pkgconf executable, while `cc`, libclang with matching resource headers, and the
  PipeWire runtime remain explicit host prerequisites. Python, containers, and Zig are not added.
- [ADR 0029: Bind capture profiles after source acquisition](0029-bind-capture-profiles-after-source-acquisition.md)
  supersedes ADR 0027 only for its profile-bearing initial lease and combined lifecycle ownership.
  Providers first return an uncalibrated lifetime lease; only an explicit immutable calibration
  binding lets the receiver emit profile-bearing `ObservedFrame` values.
- [ADR 0030: Isolate live field observation behind a run-bound worker](0030-isolate-live-field-observation-behind-a-run-bound-worker.md)
  fixes the application-owned loader, queue, provenance, result, and finish boundary between live
  screen crops and future model/catalog observers without defining accepted field values.
- [ADR 0031: Load the registered live text runtime once](0031-load-the-registered-live-text-runtime-once.md)
  binds the active catalog, PP-OCRv6-small bundle, and fixed CPU runtime manifest to one synchronous
  pre-worker loader without granting field, song, or event authority.
- [ADR 0032: Observe complete screen field sets without acceptance](0032-observe-complete-screen-field-sets-without-acceptance.md)
  fixes complete result/music-select worker outputs, explicit unimplemented fields, inference
  failure ownership, and the initial compact field-count fact without granting acceptance
  authority. ADR 0037 supersedes its exclusion of recognition values from local evidence.
- [ADR 0033: Own live field observation as one application session](0033-own-live-field-observation-as-one-application-session.md)
  joins the immutable recognition session and registered field worker under one current-run submit,
  poll, diagnostic-degradation, and ordered-finish owner without adding decision authority.
- [ADR 0034: Score every catalog song without ranking](0034-score-every-catalog-song-without-ranking.md)
  fixes a pure full-catalog comparison domain that preserves separate screen-local text scores for
  every song without ranking, truncation, acceptance, temporal state, or event authority.
- [ADR 0035: Run bounded Gamescope field observation without publishing values](0035-run-bounded-gamescope-field-observation.md)
  connects registered inference and full-catalog scoring to one bounded admitted Gamescope run,
  with a compact execution result and ordered capture, worker, and diagnostic shutdown. ADR 0037
  supersedes its discard of recognition values after counting.
- [ADR 0036: Replay recordings through the production field path](0036-replay-recordings-through-the-production-field-path.md)
  fixes the recording-source adapter and digest-bound simulation profile, removes result-background
  color from the result predicate, and observes exact `CLEAR TYPE` text through the production
  worker before any separately authorized live INFINITAS run.
- [ADR 0037: Retain recognition values in operator-owned diagnostics](0037-retain-recognition-values-in-operator-owned-diagnostics.md)
  supersedes the value-suppression parts of ADR 0032, ADR 0035, and ADR 0036. Local recognition
  evidence retains bounded OCR strings, exact catalog display/comparison strings, song IDs,
  complete candidate metrics, decisions, reasons, and expected-versus-observed values; compact
  command output remains a separate contract.
- [ADR 0038: Resolve result songs from retained simulation evidence](0038-resolve-result-songs-from-retained-simulation-evidence.md)
  fixes the first fail-closed result-song resolver from retained OCR/candidate evidence, binds
  expected song IDs in recording profile v2, and requires exact song plus `CLEAR TYPE` agreement
  before a simulation episode passes.
- [ADR 0039: Record live recognition evidence off the capture loop](0039-record-live-recognition-evidence-off-capture-loop.md)
  adds a capacity-two writer for exact live field/resolver evidence, distinguishes live monotonic
  intervals from recording PTS, and makes complete no-drop artifact persistence part of the new
  value-evidence gate without blocking or changing recognition.
- [ADR 0040: Run a foreground live recognition session](0040-run-a-foreground-live-recognition-session.md)
  adds the ordinary one-provider foreground runtime, continuous exact-value NDJSON observations,
  stdin-control-driven ordered shutdown, and non-interfering diagnostic/artifact degradation without
  granting accepted event authority.
- [ADR 0041: Register Gamescope-vkCapture recordings as a separate profile](0041-register-gamescope-vkcapture-recordings-as-a-separate-profile.md)
  binds the Gamescope-vkCapture/OBS route to its own provenance and canonical normalizer while
  retaining the three-episode requirement for complete recording simulation.
- [ADR 0042: Use measured result-panel edge rows](0042-use-measured-result-panel-edge-rows.md)
  keeps the fail-closed edge threshold and corrects the two measured result-panel rows across the
  direct Wayland frame and the independent three-episode recording profile.
- [ADR 0043: Retain bounded foreground failure evidence](0043-retain-bounded-foreground-failure-evidence.md)
  compacts hours-long foreground diagnostics and recognition evidence while pairing only selected
  result-transition or partial-result canonical frames with exact raw BGRx transform evidence.
- [ADR 0044: Require the fixed music-select label](0044-require-the-fixed-music-select-label.md)
  adds a measured fixed-label anchor to the two aggregate color predicates so startup palette
  animations fail closed instead of entering the music-select field path.
- [ADR 0045: Use measured result text regions](0045-use-measured-result-text-regions.md)
  replaces the truncated result artist region and the low-resolution result title region from
  retained foreground evidence while keeping the OCR and acceptance contracts unchanged.
- [ADR 0046: Resolve music selection from the active title prefix](0046-resolve-music-select-from-active-prefix.md)
  makes the clipped one-line active list title primary song evidence, uses strong central-title and
  artist evidence only for corroboration or tie narrowing, and rejects weighted score fusion.
- [ADR 0047: Operate target machines from private bundles](0047-operate-target-machines-from-private-bundles.md)
  makes a same-operator private bundle, explicit marker calibration, ordinary profile-selected run,
  bounded local evidence, and offline replay/update round trip the first cross-machine usage path.
  ADR 0048 supersedes its transform-first checkpoint, duplicate problem-report tail, and
  per-invocation complete bundle verification.
- [ADR 0048: Trust operator-owned artifacts across local stages](0048-trust-operator-owned-artifacts-across-local-stages.md)
  removes unconditional repeated full reads and cross-artifact re-adjudication for trusted local
  artifacts, and supersedes ADR 0047's transform-first, duplicate-tail, and per-run full-bundle
  verification requirements.
- [ADR 0049: Distribute the CLI with cargo-dist](0049-distribute-the-cli-with-cargo-dist.md)
  supersedes ADR 0047's custom deployment unit and ADR 0048's remaining private-bundle lifecycle.
  Cargo-dist produces a standard local Linux x86-64 archive and checksum, while private resources
  remain separately managed operator data.
- [ADR 0050: Cache and fetch the registered small model globally](0050-cache-and-fetch-the-registered-small-model.md)
  supersedes the runtime auto-download prohibition in ADR 0003/0006, the caller-supplied model
  location in ADR 0031, and ADR 0049's manual model-transfer requirement. Catalogs remain operator
  data; the fixed official small model is an automatically reacquired XDG cache.
- [ADR 0051: Guide local Gamescope profile setup](0051-guide-local-gamescope-profile-setup.md)
  replaces manual capture-binding transfer and routine binding/provenance arguments with a
  scorepeek-owned marker setup, one canonical machine-local profile file, profile listing, and a
  profile-selected ordinary run. Target-machine qualification remains separate.
- [ADR 0052: Watch Gamescope without owning it](0052-watch-gamescope-without-owning-it.md)
  supersedes ADR 0040's single-session, no-reconnect and stdin-stop contract and ADR 0051's
  ordinary-run lifecycle. Setup retains ownership of its dedicated calibration Gamescope only.
- [ADR 0053: Follow operator-owned local symlinks](0053-follow-operator-local-symlinks.md)
  supersedes ADR 0014's blanket local-symlink rejection for the distributed CLI. Operator-selected
  roots and inputs follow links while create-only publication and owned destructive cleanup remain
  no-clobber and non-following.
- [ADR 0054: Measure Gamescope profile transforms](0054-measure-gamescope-profile-transforms.md)
  supersedes ADR 0029/0051 requirements for launch provenance, aspect-fit geometry, complete marker
  comparison, retained launch metadata, and Gamescope-version runtime rejection.
- [ADR 0055: Bound canonical sampling footprints](0055-bound-canonical-sampling-footprints.md)
  supersedes ADR 0054's continuous-rectangle containment rule. Signed source origins are accepted
  when every canonical pixel-center sample required by the production normalizer remains present.
- [ADR 0056: Use 10 Hz diagnostics as the frame-corpus boundary](0056-use-10-hz-diagnostics-as-the-frame-corpus-boundary.md)
  makes verified diagnostic sessions the only capture-regression input, fixes recognition at 10 Hz,
  stores bounded QOI evidence plus session NDJSON streams, and makes video optional auxiliary input.
- [ADR 0057: Retry pre-admission and preserve partial live evidence](0057-retry-pre-admission-and-preserve-partial-live-evidence.md)
  retries a unique source until session admission succeeds while retaining the no-reconnect rule
  after session start, and preserves self-contained recognition observations when partial recording
  omitted their predicate facts.
- [ADR 0058: Separate run observations from terminal presentation](0058-separate-run-observations-from-terminal-presentation.md)
  supersedes ADR 0052's stdout contract by moving provisional recognition observations to a bounded
  Unix socket while TTY stdout renders watcher state and catalog-backed song resolution.
- [ADR 0059: Stabilize provisional result observations over time](0059-stabilize-result-observations-over-time.md)
  supersedes ADR 0024 only for excluding result-local temporal state and ADR 0038 only for excluding
  a provisional post-resolver result state. Raw observations remain unchanged; music-select dwell
  and accepted event authority remain unimplemented.
- [ADR 0060: Measure music-select motion before adding dwell](0060-measure-music-select-motion-before-dwell.md)
  adds an immutable operator-review draft with separate right-list, active-row, and central-title
  motion evidence. It does not label spans or implement music-select temporal state.
- [ADR 0061: Label music-select motion by adjacent pair](0061-label-music-select-motion-by-adjacent-pair.md)
  supersedes ADR 0060 only for its span-level label unit, applying digest-bound operator intervals
  to exact eligible adjacent pairs while retaining predicate transitions as context. Partial review
  remains explicitly incomplete and adds no dwell.
- [ADR 0062: Let operators exclude screen-predicate false positives from motion truth](0062-let-operators-exclude-screen-predicate-false-positives.md)
  supersedes ADR 0061 only for requiring every predicate-eligible pair to carry a motion state.
  Explicit operator screen context remains unknown and outside later motion or dwell denominators.
- [ADR 0063: Prioritize selection identity in music-select motion review](0063-prioritize-selection-identity-in-motion-review.md)
  fixes deterministic precedence when selection identity changes while the right list also moves:
  selection change wins, then same-selection list motion is scrolling, then unchanged list and
  identity are stationary.
- [ADR 0064: Reject time-only music-select dwell](0064-reject-time-only-music-select-dwell.md)
  replays session-bound OCR through the exact catalog generation and production resolver, then
  rejects all 100--500 ms candidates because each retains false stability and misses selection
  resets. ADR 0065 supersedes its reviewed-set, evaluation artifacts, and those conclusions.
- [ADR 0065: Correct selection boundaries before dwell selection](0065-correct-selection-boundaries-before-dwell-selection.md)
  moves two prematurely labeled selection changes to the adjacent pair where the visible active
  identity actually changes, replaces false-stability naming with neutral nonstationary activity,
  and finds zero missed resets. Motion truth still cannot select a runtime dwell policy.
- [ADR 0066: Evaluate the leading music-select dwell with correct-song truth](0066-evaluate-leading-music-select-dwell-with-correct-song-truth.md)
  ranks 200 ms as the leading motion candidate, binds every stationary run to an operator-reviewed
  song or non-song selection, and finds no wrong ID to suppress but additional unresolved latency.
  Runtime selection remains deferred.

## Historical

- ADR 0001: upstream release/resource adoption
- ADR 0003: Python upstream-resource importer and Rust runtime
- ADR 0008: route-local normalizers mapped to an underdetermined conceptual
  canonical frame and source ingest prematurely bound layout
- ADR 0023: explicit play-attempt linkage and full-session timeline proposal

Historical ADRs describe the initial bootstrap design and are not implementation
requirements after their named superseding decisions.
