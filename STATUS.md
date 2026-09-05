# scorepeek committed checkpoint

This file describes only the state included in its commit. Uncommitted changes are outside the
checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver/Gamescope observed-frame profile and M4 canonical recognition,
  evidence-first attempt resolution, and versioned event API remain in progress.
- RESULT v2 remains the confirmed play/history contract. MUSIC SELECT best snapshot v1 is a
  separate supplemental observation, not a play. ADR 0120 adds local score persistence;
  query CLI remains outside scope. ADR 0122 adds independent Wayland/OBS live overlays.
- The public live API is socket/snapshot v1. Diagnostic protocols remain run-event v11 and recognition observation v22.
  Joined sessions and private attempt labels remain v5. Readers retain supported older shapes and
  reject unknown versions.

## Implemented authority

- ADR 0125 replaces the fixed overlay cards/layout flags with independently positioned status,
  selection, score, history-list and history-graph widgets. ADR 0127 advances the strict overlay
  TOML to schema v2 and adds screen-aware canvases.
  `--overlay-wayland` and `--overlay-obs` enable the backends; `--overlay-config` selects the document.
  Missing configuration creates status, MUSIC SELECT, DECIDE/PLAY and RESULT canvases per backend.
  Each canvas has optional semantic-screen filters and z-order; Wayland also has 1–100% content
  opacity. UNKNOWN and socket loss retain the previous screen for the configured global grace. The
  parent is the sole atomic writer; per-canvas leases/revisions and backend-list revisions reject
  stale concurrent edits.
  Wayland owns one interactive surface per enabled native canvas and shares one feed/visibility
  clock across them. Hidden surfaces are transparent, idle and have an empty input region. OBS
  exposes stable `/canvas/<id>` URLs plus the full-screen `/overlay` multi-canvas Browser Source.
  Browser Source Interaction edits the same canvas page. Right-click enters; DONE exits. Editing a
  compact native canvas temporarily expands and repositions its surface inside the selected output;
  `/overlay` likewise promotes the edited iframe to the full stage. DONE restores saved geometry.
- CYAN SYSTEM, RESULT AURORA and DJ BLACKBOX are canvas-level skins using the approved embedded frame
  artwork through CSS backgrounds. Oxanium and OFL 1.1 are embedded alongside Japanese system-font
  fallbacks. Runtime values remain text/SVG; result emphasis is finite and the settled DOM is idle.
- Score/history widgets read only committed SQLite state. BEST integrates RESULT and SELECT sources;
  representative RESULT ordering is highest EX score, known/lower miss, then latest receipt time.
  History rows include DJ LEVEL. The graph uses exact timestamps, labeled DJ LEVEL thresholds and a
  fixed 0-100% MISS RATE axis, clipping larger MISS ratios and leaving unknown values disconnected.

- RESULT temporal acceptance compares the mandatory song/chart, clear, EX and judgment tuple.
  Supplemental/reference changes do not revoke it (ADR 0083/0087); once accepted, repeated
  supplemental payloads may update the presentation without blocking close-time confirmation.

- ADR 0126 adds a public `result_ingest_changed` lifecycle and nullable snapshot slot. With scores
  enabled, RESULT begins processing; committed or duplicate DB success becomes persisted, while
  recognition, persistence, timeout or interruption failures remain failed until DECIDE/PLAY clears
  them. Status adds nullable score and recording readiness. Unknown additive v1 events are ignored
  only after envelope and sequence validation. This does not make the public RESULT a DB authority.
- Overlay consumers still do not initialize recognition or own capture resources. Children receive
  invocation/socket/DB/config and terminate on parent-pipe EOF; one overlay failure does not stop
  recognition, persistence or its peer. No overlay flag preserves the overlay-free behavior.

- ADR 0120 adds `scorepeek-scores` as an independent public event v1 consumer. Normal run saves to
  the XDG data score database; `--scores-db` selects an instance and `--no-scores` disables it.
  Confirmed RESULTs are deduplicated history. SELECT-only charts have best rows without plays;
  SELECT retains per-field current supplements, not revision history. Later known/no-record values
  can correct supplements, while RESULT/previous-best sources retain cumulative bests.
- Integrated chart bests retain per-field provenance and are recomputed after SELECT corrections.
  Public events carry an immutable `emitted_unix_ms` notification timestamp. SQLite transactions,
  WAL/FULL, bounded worker admission and bounded drain separate committed from unsaved data.
  Socket failures no longer freeze public projection or score delivery. Save failures stop score
  admission without changing recognition; run status and opt-in diagnostic health show degradation.

- The registered PP-OCRv6-small and private fixed-cell HOG/MLP bundles remain the only text/numeric
  runtimes. Capture is canonical contiguous RGB8 1920x1080. Gamescope PLAY uses the independently
  measured BPM-outline screen-path layout v4 (ADR 0121), covering both SP graph positions; SELECT
  badges use the two explicitly approved crops.
- ADR 0117 replaces SELECT difficulty RGB area counts with the independently measured PLAYER 01
  outline in integrated-context layout v6. Both thin white edges must contrast with their interior;
  a single slot must meet 80% coverage and a 10-point winner margin. No extra model, template bitmap,
  dependency or temporal voting is introduced. Raw difficulty remains separate from best values.
- Raw 10 Hz screen observations and semantic episodes are separate. UNKNOWN suspends, matching
  known screens resume, and transitions drain admitted work before RESULT finalization. Shared OCR
  workers commit observations in source order. Identity uses independent song/chart factors,
  normalized family support and separate song/sibling-chart margins. SELECT and RESULT retain
  independent resolvers; best values never become identity evidence.
- Only confirmed attempts emit `scorepeek-result-detected-v2`. Provisional RESULT uses the same
  payload but a separate lifecycle and cannot increase result count. Ordered optional play options
  use the fixed label/marker and two matching observations. SELECT incumbent/successor evidence and
  latest-known difficulty hand off to the attempt after close-time drain.
- ADR 0114 adds the independently measured SELECT SCORE DATA layout v1. SCORE, MISS COUNT and clear
  type use the existing runtimes. Neutral-bright masking excludes dim leading placeholders; the
  fixed 1.0 numeric logit margin fails closed. Four measured dashes are explicit no recorded MISS;
  missing header or inconclusive OCR remains unknown. DJ rank is derived from chart notes/EX SCORE.
- ADR 0115 registers the SELECT-adapted HOG/MLP weights through runtime manifest artifact v3.
  The architecture, runtime schema v2, layouts and all acceptance thresholds are unchanged.
  Digest-bound provenance identifies the parent, private supervision, retention teacher and recipe.
- Best fields need two equal consecutive observations independently. Partial snapshots are allowed.
  Current-frame song/mode/difficulty must agree with resolved identity. ADR 0118 retains the
  interval and last publication across missing evidence while resetting field streaks and stopping
  adoption. Contrary credible song/mode/difficulty evidence ends the interval and clears best. Duplicate/reversed or pre-resume frames cannot update best; suspension retains publication but resets field streaks,
  closing blocks supplemental emission, and SELECT exit clears it. First and changed content emit;
  revisit starts a new observation. No achievement date, play count, option or common-play relation
  is inferred. Admitted identity evidence still drains during suspension/close; supplemental
  suppression never discards identity evidence.
- Resolver notifications compare semantic state: resolved clock/streak updates alone do not emit.
  Internal observations remain fresh; connecting-client snapshots use the last published state.
  Held identity, current stabilization and the last published revision are distinct in the TUI.
- ADR 0119 promotes `v1.sock` to the public snapshot/live NDJSON API. Confirmed/provisional RESULT,
  current selection, supplemental SELECT best and operational status have a separate typed projection,
  public sequence, event identity and session binding. Raw observations, candidates, resolver state,
  timing, recording paths and history arrays remain internal. The observation socket is removed;
  TUI, run-event v11 recording and headless replay continue to use their existing internal contracts.
- Snapshot and publication share the sequence boundary. Queue overflow disconnects existing clients;
  slow clients are isolated. Events and snapshots are bounded to 1 MiB. Oversize or worker failure
  disables public delivery without changing recognition. Reconnect restores current state, not all
  missed plays. Detailed wire and consumer state rules are in `docs/event-api.md`.
- TUI has Watcher, Latest result, Music Select Resolver, and RESULT/attempt Resolver. At 80x25
  the four panes occupy 4/8/7/6 rows and retain all attempt gates.
- ADR 0116 restricts recording completeness to runtime persistence loss. Typed facts are written
  without duplicate semantic validation; internal binding/shape/chronology defects are no longer
  emitted as recording-drop reasons. Foreign pending jobs are rejected without degrading the run.
  Existing historical reason names remain readable and stored session completeness is unchanged.
- `--record` remains opt-in for capture/recognition/events/canonical artifacts. Without it, live
  state and domain behavior operate without recording. Runtime recording loss changes completeness only.
  Canonical retained frames are lossless RGB Matroska segments with a shared memory account and
  typed intentional gaps. Successful joined publication clears owned staging; failures retain
  diagnostics. No live target, remote storage policy, or external service was changed here.
- Private corpus import uses immutable local metadata and optional S3 segments, digest-bound ranged
  downloads, bounded shared replay workers/memory, production recognition/reducers and the existing
  accepted-result oracle. Supplemental snapshot counts are reported separately per session. Optional `--trace-dir` saves
  state/domain events (excluding raw field candidates), with a shared 256 MiB budget, no overwrite,
  code/model/layout binding and non-interfering recording failure status.

## Verification

- Schema-v2 defaults/rejection, screen filters, semantic-screen snapshot/live folding,
  suspension/disconnect grace and immediate known-screen replacement have focused development-host
  tests. Workspace compilation covers the Wayland cursor-shape and generated fallback cursor,
  empty input regions, shared feed, content opacity, preview lease transfer and OBS stage routes.
  Target visual/interaction checks for these additions remain outstanding.

- Overlay development-host verification covers all three skin DOMs, embedded PNG decode, fixed
  widget bounds and settled animation. A production native headless render confirms Japanese/Latin
  text, selection rail, DB-derived BEST/DETAIL, DJ LEVEL history and graph dots/thresholds. Strict
  TOML, missing-file creation, invalid-canvas isolation, atomic save, lease/revision conflict,
  backend canvas management, local-time formatting and readback triggers have focused tests.
  The browser WASM type-checks and the real dx bundle contains served JS/WASM/font/artwork with correct
  MIME types; embedded-asset and child-EOF tests pass. `wasm-opt` still reports unsupported DWARF and
  the bundle proceeds without that optional optimization.
- Workspace/all-target Clippy passes. The complete workspace suite passes: 506 library,
  322 binary, 128 corpus library, 5 corpus binary, 25 overlay, 3 handle, 4 overlay-UI, 3 overlay-web
  and 13 score tests, plus doctests. The embedded-web overlay suite has 26 tests. The 99 offline OCR tests and
  repository checks also pass. A subsequent focused rerun passes all 5 public API tests, including
  the added semantic-screen phase contract, plus the overlay and embedded-web suites.
- Target investigation found two connected outputs while the initial Wayland canvas omitted
  `output`; the previous child rejected that multi-output state before creating a surface. The
  native child now selects the first named connected output in stable name order when `output` is
  absent or stale, persists the resolved name through the parent, retries transient save conflicts,
  and releases an active editor lease when its surface exits. Focused fallback tests, workspace
  all-target Clippy and the complete suite pass on the development host.
- Commit `a90541c2c4d9cc93327dcaa2332ba16183bafc56` was built with `mise run dist:test` and
  installed on `infinitas.lan` as `/home/atty/.local/bin/scorepeek`. The installed binary SHA-256
  is `967e735c0de182a2869d14f66ea15f5291cde196b7fd629fd3817b09aa42d7be`; target readback confirms
  the overlay CLI flags and the existing numeric model remains active. No scorepeek process was
  running, and the same-directory staging and rollback files were removed after atomic replacement.

- Recorded-input reducer replay of the complete session with digest
  `193550c1c3337905122585fb868c1c8831be3fab835c5ec9e5c03ef70c419594` confirms four results,
  matching the four private visual labels. Restoring the old full-performance equality alone
  reproduces three results: a final MISS `not_displayed` to `unknown` observation revokes the first.
  The other three payloads are unchanged. This reuses recorded field/semantic inputs through the
  production reducer; it is not fresh OCR, full corpus replay, target installation or live validation.
  Candidate labels remain unapplied and no score database is backfilled.
  All 504 library tests, library-only Clippy and the focused reducer tests pass.
  Root check/test currently stop in unrelated, uncommitted overlay
  formatting changes; this checkpoint does not claim an all-workspace validation pass for this fix.

- ADR 0121 production predicate evaluation covers 89 retained frames from the SP graph-position
  failure session. The inspected left-graph PLAY frame changes from UNKNOWN to PLAY; the other
  88 classifications remain unchanged (including 77 known non-PLAY controls and two PLAY frames).
  Synthetic regression covers both independently measured SP positions and rejects solid cyan
  panels at both positions. Thresholds and the exactly-one-screen gate are unchanged.
  A separate 786-frame legacy QOI comparison against the installed v3 build has zero screen
  classification changes: 41 PLAY, 182 RESULT, 75 SELECT, 30 DECIDE, four MODE and 454 UNKNOWN.
  An independently inspected DP PLAY frame also retains its classification and all outline metrics.
  `mise run check`, the complete `mise run test` and independent review pass for this fix.

- Scores tests cover production projection to SQLite readback, SELECT-only charts, later RESULT,
  downward SELECT corrections, partial/no-record fields, source ties, chart/instance separation,
  transaction rollback, schema mismatch, concurrent database initialization, bounded locks, queue
  overflow and shutdown timeout.
  Socket worker loss and database initialization failure do not affect the other consumer.

- Production Rust marker evaluation on the latest retained session processes 2,288 canonical frames.
  All 1,147 recorded SELECT observations resolve HYPER (old RGB predicate: 948 known, 199 unknown).
  This is recognition availability on an existing session, not a new-capture accuracy holdout.
  The two stationary failure spans recover all 40 prior unknowns in 112 observations.
- Additional retained legacy QOI evaluation covers 786 frames: all 75 recorded SELECT frames resolve
  (62 HYPER, 13 ANOTHER); 244 RESULT/PLAY/transition/mode controls accept no marker. Raw UNKNOWN
  frames include visible SELECT markers and are excluded from negative-screen accuracy claims.
  Ten manually labeled images cover all five SP slots and DP HYPER; all ten are accepted correctly.
  A separate 600-frame segment contains 166 SELECT frames across all five difficulties: known
  increases from 76 to 166, with no disagreement on previously known observations.
  DP other difficulties, other profiles and new-capture holdout remain unverified.

- The latest private session exposed the obsolete SELECT field-count validator: all 1,147
  successful SELECT field observations were rejected by diagnostic recording. Its canonical video
  and event stream are complete; the original partial session and private labeling draft remain
  unchanged. The new recorder persists the eight-field SELECT summary without semantic revalidation.

- Registered-model Rust production evaluation has 27 digest-bound manually labeled frames: 15 visible
  score panels and 12 transition/other-screen controls. SCORE, MISS and clear each accept 15/15;
  all 45 accepted fields are correct, with no control-frame acceptance. Includes
  recorded SP/DP SCORE/MISS, NO PLAY in SP/DP, FAILED, CLEAR, HARD CLEAR, EX HARD CLEAR and FULLCOMBO CLEAR.
  The parent accepted SCORE 11/15 and MISS 12/15; sampled digit 6 now passes the unchanged margin.
  Session groups share identical glyphs; this is not an unseen-glyph holdout. Frames and complete
  labels remain private.
- Current four-session production replay passes before and after interval/notification changes:
  22 accepted RESULTs, all labeled SELECT endpoints correct, 15,971 canonical frames. The complete
  RESULT and music-selection event streams are byte-equivalent as serialized values. Best snapshots
  remain 109 total (baseline 9/44/35/21, after 8/44/37/20); counts alone are not a correctness oracle.
- Resolver notifications decrease from 10,737 to 2,200. Adjacent notifications differing only in
  sequence/time or resolved difficulty streak decrease from 8,023 to zero. Baseline traces contain
  no same-chart adjacent snapshot restart candidates; retention correctness is also tested with
  explicit missing-evidence, conflict, mode/difficulty, suspension and delayed-job scenarios.
- Six manually reviewed frames label two short stationary intervals. Both baseline and updated
  replay have zero wrong chart/value associations and zero duplicate publications in those intervals.
  Three inspected scrolling frames have ambiguous central/list association and are excluded from
  stationary truth. These samples do not establish zero error across all scrolling transitions.
- SELECT lifecycle tests cover fresh two-observation recovery, no adoption while held, true revisit,
  contrary ambiguous candidates, existing unresolved mode conflict, stale work and result separation.
  Connecting snapshots preserve the last publication. Existing four-pane gate tests and held-state
  rendering pass at 120x40 and 80x25. Trace capacity/no-overwrite tests pass.
- `mise run check`, workspace/all-target Clippy and the complete default-parallel `mise run test`
  pass (501 runtime, 316 binary, 128 corpus library, 5 corpus binary and 11 scores tests, plus 99 offline OCR tests).
  Public API tests cover snapshot/live folding, provenance readiness, old queued-record exclusion,
  overflow with no subsequent event, idle reconnects/write-half-close, partial/slow clients, record
  limits, channel failure non-interference, and socket ownership cleanup. Raw diagnostic records and
  accepted RESULT payloads remain separate. The binding-mismatch fixture uses the existing isolated
  test supervisor. Independent final review of the API promotion has no actionable findings.
- Trace provenance binds the running executable and both SELECT layouts; the three-file hash is
  explicitly a partial source fingerprint. Full production replay covers the final reducer/recognition
  behavior. Subsequent writer-provenance and test-fixture-only corrections pass focused tests,
  the complete suite and final review. Traces, six-frame interval labels and comparison reports are
  retained privately under `select-stability-evaluation-v1` in the scorepeek XDG data directory.

## Unverified and next execution boundary

- Target-live validation is still required for screen-driven surface visibility, configured
  opacity, forced cursor shapes/fallback, peer-surface z behavior, OBS `/overlay` empty-space menu
  and hidden-canvas preview, compositor output selection/bounds, initial
  upper-right placement, cross-output drag prevention and native canvas-list/output hot reconciliation,
  integer/fractional visual equality, Gamescope foreground behavior, OBS Interaction, readability,
  CPU/GPU/OBS lag and idle render cost. Development-host DOM/headless/browser tests do not certify
  those target behaviors. Target binary installation is complete; no live run, autostart, push or
  release is included.
- Pinned dx produces a browser bundle, but its optional wasm-opt step reports unsupported DWARF
  and skips optimization. Asset MIME/type tests pass on the emitted bundle; it is not claimed to
  be wasm-opt optimized. Browser visual verification remains separate.

- Validate layout v4 in a fresh target-live run with the installed binary.
  Retained-frame inspection does not recover unrecorded PLAY spans or backfill missing RESULTs.

- Confirm the new marker on target-live sessions.
- The previously observed diagnostic store root-lease test flake remains outside this change;
  the separately reproduced binding-mismatch test supervisor conflict is corrected.
- Additional capture coverage is needed for four-digit MISS, all remaining clear labels,
  other capture profiles, and target-live cost of the additional SELECT OCR jobs. The 27-frame
  best-value evaluation is a bounded sample, not a general zero-error claim. No explicit hidden-panel pattern
  is validated; `not_displayed` exists in the type but is not guessed from blank pixels.
- Current active private regression generation is
  `c4606091f2b2ca08686f4054a6cf080fc04f66a182c5177cbe9a9685b1ff4b20`: 4 sessions, 22 accepted
  attempts and 15,971 canonical frames. The three original entries are unchanged. The latest
  complete session now has its six manually reviewed RESULT/SELECT labels formally applied.
  The original three-session suite and the additional session were replayed separately with the
  same final runtime; all four sessions passed. Developer-host replay is not target-live verification.
- Public API developer-host verification and independent review are complete. The versioned wire
  contract is ready for consumer integration. Target-live API performance and capture gates remain
  unverified. Score persistence is developer-host verified only; target-live cost, release and push
  are not included.
