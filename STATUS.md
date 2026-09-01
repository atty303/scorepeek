# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it. Uncommitted changes are
outside the checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 canonical recognition, evidence-first attempt resolution, and versioned event API: **in
  progress**.
- `scorepeek-result-detected-v2` remains the accepted public domain contract and now carries typed
  ordered `play_options`. Debug output uses run-event v6, observation socket/snapshot v6, and
  recognition observation v17. Joined recorded sessions use v5.
- Text authority remains the registered PP-OCRv6-small bundle. Numeric authority remains the
  private fixed-cell HOG/MLP model. No model bytes, real crops, complete labels, or generated
  datasets are committed.

## Implemented authority

- The 10 Hz classifier emits raw known/unknown observations independently of field-worker busy
  state. `ScreenEpisodeResolver` owns monotonic semantic episodes. Raw unknown suspends the active
  known screen; the same screen resumes it; only another known screen, session boundary, or reversed
  chronology closes it.
- Routine recording is opt-in. `scorepeek run` retains domain behavior without creating capture,
  recognition, run-event, joined-session, or canonical artifacts; `scorepeek run --record` starts
  them as one session after FFmpeg/capacity preflight. Removed recording flags fail as unknown
  options. Recorder failure changes only component/session completeness.
- Canonical recording retains every MUSIC SELECT, DECIDE TRANSITION, and RESULT tick plus the first,
  last, and raw-screen transition windows. Stable PLAY, MODE SELECT, and UNKNOWN interiors are typed
  intentional gaps. Retained RGB24 frames are streamed to external lossless `libx264rgb` Matroska
  segments, then decoded and checked against input digest and frame count before complete
  publication.
- Field jobs are bound to the semantic episode that admitted them. Episode close stops admission,
  drains already-submitted work, applies it to the closing episode, finalizes RESULT, and only then
  starts the next known-screen attempt transition. Late generation/chronology evidence remains
  diagnostic and cannot enter resolution.
- MUSIC SELECT uses incumbent/successor selection epochs rather than accepted-song arming. An
  unfinished latest successor is handed off only after close-time admitted-field drain, instead of
  copying an older incumbent at close. Select difficulty is one latest-known state per active epoch:
  one different typed-known marker replaces it immediately, unknown retains it, and difficulty-only
  frames update successor/incumbent without adding song evidence. Before any credible song, only the
  latest pending value is retained and applied once. Attempt state owns screen path and select/result evidence snapshots, not
  separate selected/result songs.
- MUSIC SELECT and RESULT retain independent title/artist song factors and difficulty/notes/level
  chart factors, then project them onto the full catalog `(song, play type, difficulty)` hierarchy.
  Chart factors survive until later song evidence arrives but cannot create a song. Empty/whitespace
  observations add no support. Cross-frame raw `u64` sums are normalized proportionally by family
  above the 300 cap. The best other song and best sibling chart have separate margins; level remains
  positive-only and never vetoes. Resolver authority retains the complete typed hierarchy across
  capture; diagnostic artifact/socket sinks construct only bounded candidate projections.
- All active-list titles use the registered foreground extractor: grayscale greater than 80,
  complete foreground bounds, four horizontal pixels of margin, and full ROI height. Foreground
  PP-OCR is lexical authority; wide OCR remains raw diagnostic evidence. Unicode scalar count and
  foreground width share the same title family, contributing the maximum correlated score rather
  than a second vote. Raw `X`
  remains `X`; no alias or song-specific correction exists.
- RESULT is provisional while displayed. Only semantic RESULT close after field drain can confirm
  an attempt and emit. Confirmation is recorded before the attempt's sole v2 domain event.
  Unresolved/conflicting identity, missing linkage/play, abandoned attempts, or incomplete required
  numeric tuples complete with typed rejection and emit no result. Direct RESULT-to-PLAY retry
  inherits the parent selection context once without re-adding frame support.
- RESULT play options use the measured whole label panel at `(30,318,530,50)`. A sixth PP-OCR text
  job reads the complete `USE OPTION ...` display; a fixed orange marker separately distinguishes a
  positively absent label from inconclusive blank OCR. The finite ordered vocabulary permits a
  unique whole-display edit distance of at most one. Two matching typed observations in the same
  semantic RESULT episode produce a known ordered list. Conflict, OCR failure, and incomplete
  evidence remain typed optional unknown and never suppress an otherwise accepted result event.
- The TUI retains one three-pane layout and semantic state palette. Watcher shows raw and semantic screen plus suspension;
  Latest domain holds only the last accepted v2 event; Resolver shows incumbent/successor/result
  evidence, foreground title geometry, hierarchical runners, family contributions, attempt path,
  and every promotion gate. MUSIC SELECT field observations update this typed snapshot; ticks keep
  the latest observation, while a new semantic episode or session clears it. Raw marker and
  resolver-current difficulty are displayed separately with the consecutive-known count. The worst-case 80x25
  tree keeps all gates visible. TUI formatting owns no resolver logic.
- Run-event v6 distinguishes raw screen observations, semantic episode transitions, current
  selection-difficulty changes, selection/result and provisional-joint transitions, attempt
  finalization, and suppression. Recognition observation
  v17 retains title views/geometry, episode binding, fixed-cell numeric evidence, play-option raw
  OCR/marker/typed state, factor support, raw stage/frame timing, late/drain status, and suppression
  evidence. Independent PP-OCR jobs use single-threaded ONNX sessions in a pool selected from
  available parallelism; the outer coordinator pipelines frames and commits admitted evidence in
  source order. Readers accept run-event v2 through v6 and recognition v5 through v17.
- The attempt corpus clean-cuts to complete joined-session v5 input and label v5 truth. Import keeps
  lossless segments and the tick index as immutable objects without QOI expansion or pixel-content
  deduplication. Replay decodes canonical retained frames only, starts no normalizer, uses production
  screen-episode and run-event reducers, and rejects missing/elided DECIDE TRANSITION or RESULT
  truth. Event comparison normalizes runtime IDs while preserving attempt-parent and ordered
  play-option relations.

## Verification boundary

- Targeted Rust library and binary suites pass after removal of production result/music-select
  temporal reducers. Regression tests require no domain event before RESULT finalization, require
  confirmed attempt before the one event, preserve family ratios, allow accepted state to return to
  conflict before close, and keep direct retry deduplicated.
- Saved target diagnostic `run-1788248141-530814846-1005386-session-1` remains outside the corpus and
  is a read-only failure oracle. Its first attempt retains select raw `A`, one-character geometry,
  long artist, HYPER, empty RESULT title, and RESULT artist/notes evidence for operator-confirmed
  `∀ / SP HYPER / 1136`. A factor-first resolver regression reaches that joint chart without OCR
  rewriting; the other three accepted attempt identities remain prospective replay checks.
- Saved target diagnostic `run-1788255215-37773013-1050141-session-1` also remains outside the
  corpus. Its same-song `X` marker cycle at source sequences 3240 through 3319 is the read-only
  failure oracle for immediate `HYPER → ANOTHER → NORMAL → HYPER → ANOTHER` current-state changes.
- The saved diagnostic is not a complete current semantic replay: retained-frame reevaluation does
  not reconstruct the original 10 Hz attempt timeline. Prospective target execution is therefore
  still required for end-to-end authority.
- The existing private corpus and active suite were not changed. The operator plans to rebuild
  them. New regression import accepts only complete joined-session v5 and label v5 with ordered
  play-option truth; there is no active-suite legacy reader or converter. The current 34 RESULT episodes are a read-only play-option oracle covering
  no option, every supported single option, and `RANDOM,LEGACY`, but do not advance the active
  suite. Title-view/support calibration and wrong-event authority still require a reviewed,
  session-disjoint replacement corpus with zero wrong joint acceptance and zero wrong events.
- Read-only whole-panel evaluation over those 34 stable QOIs produced exact registered PP-OCR text
  for every displayed option. The set includes a positively blank panel, R-RANDOM, S-RANDOM,
  MIRROR, A-SCR, two LEGACY results, and `RANDOM,LEGACY`; the orange-marker count separated the
  blank panel (0) from every nonblank example (minimum 2,288).
- The read-only active suite replay passes with 8 sessions, 34 episodes, 2,888 canonical frames,
  and one negative frame when run against an isolated temporary activation of the rebound numeric
  manifest. The temporary store was removed afterward; the operator's normal private store and
  active corpus pointer were not changed.
- The cargo-dist binary was installed hash-first on `infinitas.lan` at
  `/home/atty/.local/bin/scorepeek`; local and installed executable SHA-256 are both
  `148cfbd5687028e3d7bb4fe1bca807f3dcf3d217c8230fae182f5e5650ad07d1`. `doctor` reports the
  fixed-slot numeric model active with manifest
  `cf099b27b533a79534db62a912d7c4b4e949ac29b786f57bb5ed6f21cf7766d6`. No scorepeek process was
  running at readback time, so no stale process required restart and no `/proc/<pid>/exe` digest was
  available to compare.
- Public `/v1.sock` authority, target support, prospective target behavior, push, release, and model
  publication remain unverified boundaries.
- The canonical layout digest changed when the play-option panel was added. A later target install
  must publish/activate a private numeric manifest bound to the new layout digest while reusing the
  same model bytes; this checkpoint does not mutate the target model store or installed binary.

## Next executable task

Record a fresh joined-session v5 run, import and operator-review label v5 truth, then compare the
same suite with one text worker and the default pool. Require identical domain events plus reduced
OCR and whole-suite wall time before claiming speedup. After that, publish a numeric manifest rebound
to the new canonical-layout digest and explicitly install the binary plus
manifest on the target. In the following prospective session, verify RESULT-close option payloads,
six-job wall time/busy skips, 10 Hz raw cadence, confirmed-attempt ordering, one event per accepted
attempt, and event drop zero before changing target or public-socket authority.
