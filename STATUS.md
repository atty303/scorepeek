# scorepeek committed checkpoint

This file describes only the state included in its commit. Uncommitted changes are outside the
checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver/Gamescope observed-frame profile and M4 canonical recognition,
  evidence-first attempt resolution, and versioned event API remain in progress.
- RESULT v2 remains the confirmed play/history contract. MUSIC SELECT best snapshot v1 is a
  separate supplemental observation, not a play. DB persistence and history supplementation UI
  remain outside the implemented scope.
- Current diagnostic protocols are run-event/socket/snapshot v10 and recognition observation v21.
  Joined sessions and private attempt labels remain v5. Readers retain supported older shapes and
  reject unknown versions.

## Implemented authority

- The registered PP-OCRv6-small and private fixed-cell HOG/MLP bundles remain the only text/numeric
  runtimes. Capture is canonical contiguous RGB8 1920x1080. Gamescope PLAY uses the independently
  measured BPM-outline screen-path layout v3; SELECT badges use the two explicitly approved crops.
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
- `--record` remains opt-in for capture/recognition/events/canonical artifacts. Without it, live
  state and domain behavior operate without recording. Failed recording changes completeness only.
  Canonical retained frames are lossless RGB Matroska segments with a shared memory account and
  typed intentional gaps. Successful joined publication clears owned staging; failures retain
  diagnostics. No live target, remote storage policy, or external service was changed here.
- Private corpus import uses immutable local metadata and optional S3 segments, digest-bound ranged
  downloads, bounded shared replay workers/memory, production recognition/reducers and the existing
  accepted-result oracle. Supplemental snapshot counts are now reported separately per session.

## Verification

- Private SELECT evaluation has 27 digest-bound manually labeled frames: 15 visible score panels
  and 12 transition/other-screen controls. SCORE accepts 11/15 (73.3%), MISS 12/15 (80%), clear
  15/15 (100%); all 38 accepted fields are correct, with no control-frame acceptance. Includes
  recorded SP/DP SCORE/MISS, NO PLAY in SP/DP, FAILED, CLEAR, HARD CLEAR, EX HARD CLEAR and FULLCOMBO CLEAR.
  The registered numeric model rejects sampled digit 6 at the fixed margin; those fields remain
  unknown. Frames and complete labels are private under the operator's scorepeek data directory.
- Unit/lifecycle tests cover independent stability, rank boundaries, partial/changed/deduplicated
  snapshots, revisit identity, impossible EX SCORE, selection clearing, suspension, late work and
  separation from confirmed results. Existing socket and TUI regression tests are retained.
- Full private production replay passes: 3 sessions, 16 accepted attempts, 13,683 canonical frames,
  correct selection at every labeled SELECT span, and no wrong/missing RESULT events. Supplemental
  snapshots total 253 (9/220/24 per session). The run decoded 24 remote segments and took 439.2 s
  on the development host. Live, replay and reducer producers share the v10 schema constant.
- `mise run check`, workspace/all-target Clippy and fresh independent review pass. The live
  serializer/reducer schema regression and generated-session reader regression pass.
- The complete `RUST_TEST_THREADS=1 mise run test` passes without skipped tests (493 runtime,
  309 binary, 127 corpus library and 5 corpus binary tests, plus offline OCR validation).
  Default parallel execution encountered one `WorkerUnavailable` failure in the unchanged
  diagnostic store root-lease rebinding test; earlier parallel execution and the final serial
  execution passed. Its intermittent cause remains unverified.

## Unverified and next execution boundary

- Investigate the intermittent diagnostic store lock-test failure under parallel test execution.
- Additional capture coverage is needed for four-digit MISS, all remaining clear labels,
  other capture profiles, and target-live cost of the additional SELECT OCR jobs. The 27-frame
  evaluation is a bounded sample, not a general zero-error claim. No explicit hidden-panel pattern
  is validated; `not_displayed` exists in the type but is not guessed from blank pixels.
- Current active private regression generation is
  `d2bd843f63729327663587a7b2227ec65f064554b1192bdf4dd754cfa08ff296`: 3 sessions, 16 accepted
  attempts and 13,683 canonical frames. Developer-host replay is not target performance or live
  verification.
- `v1.sock` remains the planned accepted-event interface; current observation clients use
  `observations-v10.sock`. No result history DB, deployment, release or push was performed.
