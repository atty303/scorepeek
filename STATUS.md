# scorepeek committed checkpoint

This file describes only the state included in its commit. Uncommitted changes are outside the
checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver/Gamescope observed-frame profile and M4 canonical recognition,
  evidence-first attempt resolution, and versioned event API remain in progress.
- RESULT v2 remains the confirmed play/history contract. MUSIC SELECT best snapshot v1 is a
  separate supplemental observation, not a play. DB persistence and history supplementation UI
  remain outside the implemented scope.
- Current diagnostic protocols are run-event/socket/snapshot v11 and recognition observation v22.
  Joined sessions and private attempt labels remain v5. Readers retain supported older shapes and
  reject unknown versions.

## Implemented authority

- The registered PP-OCRv6-small and private fixed-cell HOG/MLP bundles remain the only text/numeric
  runtimes. Capture is canonical contiguous RGB8 1920x1080. Gamescope PLAY uses the independently
  measured BPM-outline screen-path layout v3; SELECT badges use the two explicitly approved crops.
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
  Current-frame song/mode/difficulty must agree with resolved identity. Changed/unknown identity
  clears best. Duplicate/reversed or pre-resume frames cannot update best; suspension retains state,
  closing blocks supplemental emission, and SELECT exit clears it. First and changed content emit;
  revisit starts a new observation. No achievement date, play count, option or common-play relation
  is inferred. Admitted identity evidence still drains during suspension/close; supplemental
  suppression never discards identity evidence.
- `music_select_best_observed` and typed `music_select_resolver_changed` share the observation
  connection, connecting-client snapshot and headless replay. The public supplemental payload
  excludes raw OCR/candidates. TUI has Watcher, Latest result, Music Select Resolver, and
  RESULT/attempt Resolver. At 80x25 the four panes occupy 4/8/7/6 rows and retain all attempt gates.
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
  accepted-result oracle. Supplemental snapshot counts are now reported separately per session.

## Verification

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
- The newly labeled latest session passes production replay separately: six accepted RESULTs,
  all labeled SELECT endpoints correct, 2,288 canonical frames, 46 supplemental snapshots
  (recorded baseline 73), and no wrong/missing RESULT events.
- Unit/lifecycle tests cover independent stability, rank boundaries, partial/changed/deduplicated
  snapshots, revisit identity, impossible EX SCORE, selection clearing, suspension, late work and
  separation from confirmed results. Existing socket and TUI regression tests are retained.
- Registered-model full private production replay passes: 3 sessions, 16 accepted attempts, 13,683 canonical frames,
  correct selection at every labeled SELECT span, and no wrong/missing RESULT events. Supplemental
  snapshots total 64 (7/37/20 per session; pre-outline registered-model baseline 255). The run decoded
  24 remote segments and took 478.9 s
  on the development host. Live, replay and reducer producers share the v11 schema constant.
- `mise run check`, workspace/all-target Clippy and fresh independent review pass. The live
  serializer/reducer schema regression and generated-session reader regression pass.
- The complete `RUST_TEST_THREADS=1 mise run test` passes without skipped tests (485 runtime,
  300 binary, 127 corpus library and 5 corpus binary tests, plus 99 offline OCR tests).
  ADR 0116's SELECT persistence, rejected foreign inputs and retained runtime-loss cases pass;
  `mise run check`, workspace/all-target Clippy, fresh review and follow-up review also pass.
  Default parallel execution encountered one `WorkerUnavailable` failure in the unchanged
  diagnostic store root-lease rebinding test; earlier parallel execution and the final serial
  execution passed. Its intermittent cause remains unverified.
- The new registration passes the ordinary installer in an isolated model store, six numeric
  manifest/runtime tests, the full serial workspace suite and fresh independent review. Private
  weights and generation records are retained in the operator's private artifacts. The deployed
  binary and live model store remain unchanged.

## Unverified and next execution boundary

- Confirm the new marker on target-live sessions; target install is a separate operation.
- Investigate the intermittent diagnostic store lock-test failure under parallel test execution.
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
- `v1.sock` remains the planned accepted-event interface; current observation clients use
  `observations-v11.sock`. No result history DB, deployment, release or push was performed.
