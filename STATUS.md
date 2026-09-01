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
  segments. One shared configurable memory account defaults to 1024 MiB; its input channel has no
  independent item capacity. Limit rejection or encoder failure degrades recording without changing
  domain processing, and the TUI exposes current/limit/high-water bytes and frame loss. Intentional
  gaps stay inside a segment. The realtime path records input and encoded digests but defers decode
  and RGB digest verification to corpus verify/import.
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
- Routine `--record` capture diagnostics are facts-only and retain no legacy canonical/source QOI
  pixels beside the canonical segment. `session_finished` moves recording to finalizing; an explicit
  `recording_ready` is emitted only after atomic joined-session publication, so that immutable
  session is importable while the watcher continues.

## Verification boundary

- The target session `run-1788272474-298014477-1183660-session-1` is the recorder failure oracle:
  5,626 recognition ticks produced 5,536 canonical tick records and 90 queue drops. The first large
  segment contained 563 frames; synchronous segment decode in the recorder stalled its two-frame
  queue, and each resulting sequence gap forced another close. The current implementation removes
  both causes. Local synthetic coverage now verifies the shared-limit recovery/sticky degradation,
  retention and chronology-reset windows, streamed tick-index digest/count, facts-only QOI
  suppression, recording health/finalizing/ready lifecycle, broken encoder stdin cleanup, and
  lossless FFmpeg round trip. Corpus decode now has bounded timeout/kill/reap behavior, and segment
  binding follows retained frame order; a chronology reset fails the sequence-only label-v5
  completeness boundary. After the first encoder failure, already-admitted frames drain as loss
  without repeated child restarts. Numeric dataset authoring now consumes the same segment/tick
  iterator as replay instead of treating a segment-backed frame as QOI. Canonical import requires
  the joined tick artifact and rejects sequence/time reversal before object publication; encoder
  early failures reap the child and remove unpublished output. Recording lifecycle events retain
  session/generation binding through run-event serialization; canonical v2 memory/integrity fields
  are fail-closed, tick parsing is streaming bounded, and numeric authoring uses two decode passes
  per session. `mise run check`, pedantic workspace clippy, and the complete serial `mise run test`
  suite pass after these final review fixes. Parallel
  full-suite
  runs exposed three existing `diagnostic_control` worker-availability flakes; every failing test
  passed when rerun alone. Fresh target and real import timing verification is still pending.

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
- The shared-memory/deferred-verification recorder revision was built through the cargo-dist path and
  installed hash-first on `infinitas.lan` at `/home/atty/.local/bin/scorepeek`; local, transferred,
  and installed executable SHA-256 are all
  `9be36525987e9565e30a41fe20f02763037778f25602d12115d2c267bfa09057`. `doctor` reports the
  fixed-slot numeric model active with manifest
  `5e5b545d57a6197f4aaa6a863595f237cb19095903baa135960e4c257cda2137`. No scorepeek process was
  running before or after replacement, so no stale process required restart and no
  `/proc/<pid>/exe` digest was available to compare. Target recording behavior and the
  recording-ready lifecycle remain unverified.
- Public `/v1.sock` authority, target support, prospective target behavior, push, release, and model
  publication remain unverified boundaries.
- The canonical layout digest changed when the play-option panel was added. A later target install
  must publish/activate a private numeric manifest bound to the new layout digest while reusing the
  same model bytes; this checkpoint does not mutate the target model store or installed binary.

## Next executable task

Record a fresh joined-session v5 run on the installed target revision and confirm no frame-admission
loss, visible memory health, facts-only capture diagnostics, and
`recording_ready` after session end. Import and operator-review label v5 truth, then compare the
same suite with one text worker and the default pool. Require identical domain events plus reduced
OCR and whole-suite wall time before claiming speedup. After that, publish a numeric manifest rebound
to the new canonical-layout digest and explicitly install the binary plus
manifest on the target. In the following prospective session, verify RESULT-close option payloads,
six-job wall time/busy skips, 10 Hz raw cadence, confirmed-attempt ordering, one event per accepted
attempt, and event drop zero before changing target or public-socket authority.
