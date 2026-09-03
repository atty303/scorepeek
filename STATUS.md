# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it. Uncommitted changes are
outside the checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 canonical recognition, evidence-first attempt resolution, and versioned event API: **in
  progress**.
- `scorepeek-result-detected-v2` remains the accepted public domain contract and now carries typed
  ordered `play_options`. The same result-content payload is used by the non-authoritative
  `result_provisional_changed` lifecycle. Debug output uses run-event v8, observation
  socket/snapshot v8, and
  recognition observation v19. Joined recorded sessions use v5.
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
  options. In-progress components are grouped by session under `recording-staging/` and successful
  joined publication removes that complete tree; failed publication retains it for diagnosis.
  Published sessions remain under `diagnostic-sessions/`. No separate watcher-status file is
  written. Recorder failure changes only component/session completeness.
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
- RESULT is provisional while displayed. Accepted joint identity plus two matching numeric
  observations emit a revisioned provisional v2 payload when an active attempt ID exists. Payload
  change replaces it; identity/numeric loss, close-time rejection, or session end withdraws it.
  Only semantic RESULT close after field drain can confirm an attempt and emit `result_detected`.
  Confirmation is recorded before the attempt's sole confirmed v2 domain event, using the same
  payload builder and no intervening withdrawal.
  Unresolved/conflicting identity, missing linkage/play, abandoned attempts, or incomplete required
  numeric tuples complete with typed rejection and emit no result. Direct RESULT-to-PLAY retry
  inherits the parent selection context once without re-adding frame support.
- RESULT play options use the measured whole label panel at `(30,318,530,50)`. A sixth PP-OCR text
  job reads the complete `USE OPTION ...` display; a fixed orange marker separately distinguishes a
  positively absent label from inconclusive blank OCR. The finite ordered vocabulary permits a
  unique whole-display edit distance of at most one. Two matching typed observations in the same
  semantic RESULT episode produce a known ordered list. Conflict, OCR failure, and incomplete
  evidence remain typed optional unknown and never suppress an otherwise accepted result event.
- RESULT play type uses integrated context layout v4 ROI `(925,1025,75,50)`, measured across all
  five SP difficulty layouts with left width reserved for a DP glyph. Exact `SP`/`DP` OCR is typed;
  one type needs two observations and no opposite-type observation in the semantic RESULT episode
  before chart identity can be accepted and it contributes an independent chart family. Once
  known, opposite-type candidates are excluded. Field-local chart resolution no longer supplies
  SP. DP image recognition remains unverified because the active corpus contains SP only.
- The TUI retains one three-pane layout and semantic state palette. Watcher shows raw and semantic screen plus suspension;
  Latest result prefers an active `PROVISIONAL` payload and falls back to the last `CONFIRMED` v2
  event after withdrawal; only confirmed events enter count/history. Resolver shows incumbent/successor/result
  evidence, foreground title geometry, hierarchical runners, family contributions, attempt path,
  and every promotion gate. MUSIC SELECT field observations update this typed snapshot; ticks keep
  the latest observation, while a new semantic episode or session clears it. Raw marker and
  resolver-current difficulty are displayed separately with the consecutive-known count. The worst-case 80x25
  tree keeps all gates visible. TUI formatting owns no resolver logic.
- Run-event v8 additionally retains resolved/update/withdraw provisional RESULT lifecycle in the
  bounded diagnostic artifact, observation socket/snapshot, and headless replay. It otherwise
  distinguishes raw screen observations, semantic episode transitions, current
  selection-difficulty changes, selection/result and provisional-joint transitions, attempt
  finalization, and suppression. Recognition observation
  v19 retains title views/geometry, episode binding, fixed-cell numeric evidence, play-option and
  play-type raw
  OCR/marker/typed state, factor support, raw stage/frame timing, late/drain status, and suppression
  evidence. Independent PP-OCR jobs use single-threaded ONNX sessions in a pool selected from
  available parallelism; the outer coordinator pipelines frames and commits admitted evidence in
  source order. Live uses half the available parallelism capped at twelve; offline replay uses one
  global pool of available parallelism minus four capped at twelve. Readers accept run-event v2
  through v8, reject unknown v9, and accept recognition v5 through v19.
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
- Canonical corpus replay initially queues only suite index and object digests and has no fixed
  session limit. It admits bounded active session state, runs one single-threaded FFmpeg child per
  scheduled segment, and returns the session to a FIFO after reap so another ready session can use
  the slot. Close-time drain runs on a separate finalizer pool, so it cannot consume a decoder slot;
  failed sessions stop admitted recognition work before their memory reservation is released.
  Decoder count derives from one quarter of available parallelism and the 2048 MiB default
  memory account. Sessions share one registered OCR pool; timeline, ordered commit, attempt state,
  and final event comparison remain session-local. `--text-workers` and `--memory-mib` provide
  bounded explicit replay controls. Summary v3 reports active/blocked/completed sessions, actual
  decoder overlap, child and per-session wall time, memory/decoder/ordered-commit waits, tracked
  memory, process RSS, and aggregate FFmpeg RSS.

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
- The legacy private corpus was explicitly retired and its 8,233,757,476-byte
  `private-corpus-v1` root was deleted after the replacement replay passed. The active
  `private-corpus-v2` generation is
  `112bb422399a3702920e31df150b7a6b678aae3f9b203ccdec2ec5e3be4f11b4`; it contains two complete
  target sessions and fourteen operator-reviewed accepted attempts. The first session covers every
  SP difficulty of `Mind Mapping / Ryu☆`; the second includes the `Wizards!` SP HYPER sibling-chart
  failure oracle and nine accepted attempts. There is no legacy reader, converter, or retained QOI
  corpus.
- The v4 play-type path passes the complete active generation with 2 sessions, 14 accepted
  attempts, 12,460 canonical frames, and zero negative frames. Default offline policy used twelve
  text workers, eight preprocess workers, two concurrent decoders, and a 2 GiB tracked-memory
  account. Corpus wall time was 373,844,534 microseconds; tracked memory peaked at 1,728,053,248
  bytes. Process RSS peaked at 5,227,995,136 bytes, so this local replay is correctness evidence,
  not a target resource or performance pass.
- The provisional RESULT lifecycle has targeted coverage for identity-only, one numeric
  observation, missing attempt ID, payload deduplication/replacement, identity and numeric
  conflict withdrawal, re-resolution revision order, linkage-deficient final rejection, successful
  confirmation ordering and exact payload equality, TUI fallback, socket/snapshot delivery,
  diagnostic artifact retention, and recording-failure noninterference. `mise run check`, pedantic
  workspace clippy, the complete serial `mise run test` suite (474 scorepeek library, 295 scorepeek
  binary, and 102 scorepeek-corpus library tests), and the active private generation replay pass.
  Replay retained 2 sessions, 14 accepted episodes, 12,460 canonical frames, zero negative frames,
  and unchanged confirmed-event comparison. It used an isolated current numeric-manifest
  activation; the developer's normal active pointer was unchanged afterward.
- After independent review, the result resolver provenance is versioned as v6, play-type authority
  activation is part of resolver transition identity, the exact uppercase parser rejects case and
  Unicode-whitespace drift, and the unchanged result-crop artifact v2 remains a 20-crop contract.
  The prior play-type revision passed 465 scorepeek library tests, 286 serial scorepeek binary
  tests, and 101 scorepeek-corpus library tests.
- Historical read-only whole-panel evaluation over the retired corpus's 34 stable QOIs produced exact registered PP-OCR text
  for every displayed option. The set includes a positively blank panel, R-RANDOM, S-RANDOM,
  MIRROR, A-SCR, two LEGACY results, and `RANDOM,LEGACY`; the orange-marker count separated the
  blank panel (0) from every nonblank example (minimum 2,288).
- The new active suite replay passes after full segment decode with one session, five attempts,
  1,061 retained canonical frames, zero negative frames, and equal domain-event results in both the
  one-worker and default-pool configurations. A one-worker run measured 88,075,495 microseconds of
  text-batch wall time and 106,918,957 microseconds corpus wall time; the 31-worker default pool
  reduced text-batch wall time to 27,697,130 microseconds but increased corpus wall time to
  142,942,802 microseconds. The OCR speedup gate therefore remains failed: worker-pool sizing or
  startup/contention cost must be corrected before claiming whole-corpus acceleration. A fresh
  one-worker replay after deleting the legacy root and transfer staging still passed in
  101,232,925 microseconds, proving the active v2 objects are independently replayable.
- The development host is a Ryzen 9 9950X3D exposing 32 online logical CPUs, affinity and cpuset
  `0-31`, `cpu.max=max`, and no observed cgroup CPU pressure. The earlier statement that it exposed
  roughly two effective CPUs was incorrect: one-worker replay consumed about 200 CPU-seconds and
  multiple workers raised process utilization above 300%, while the single-session producer path
  saturated before the machine. Pure decode of the two segments measured about 14.7 seconds.
  Replay now sends pure classification/crop preparation to an eight-worker global pool, runs up to
  four frame-local outer field workers per offline session (two live), removes ordinary-title
  edit-distance heap allocation, and retains source-ordered commit and finalization. Three
  current-path runs preserve all five accepted events in both configurations: the one-worker
  median is 86,649,067 microseconds and the twelve-worker median is 31,214,081 microseconds. The
  pooled median is 64.0% below one worker and 46.1% below the prior 57,920,114-microsecond default
  median, but still misses the 20-second gate. Across the three pooled runs, measured tracked memory
  peaks at 687,865,856 bytes and process RSS at 1,793,572,864 bytes. Replay summary v3 now
  separates decoder-consumer, preparation, field queue, text/numeric, join, catalog, and ordered
  commit durations from FFmpeg child lifetime. Before this revision, a production-outer-scheduler
  fixture referencing the immutable session under four distinct session keys replayed 4,244 frames
  in 71,330,876 microseconds: four decoders overlapped,
  eight two-segment children were reaped, reports remained in corpus order, tracked memory peaked
  at 1,560,281,088 bytes under the 2 GiB account, and throughput was 3.29 times the final
  58,670,445-microsecond single-session verification. A separate synthetic FFmpeg integration
  verifies live PID overlap and per-child RSS/lifecycle
  reporting.
- Option-free live and corpus execution now always selects its CPU-derived production worker policy.
  The hidden `SCOREPEEK_INTERNAL_SINGLE_TEXT_WORKER` comparison override is removed; benchmark and
  diagnostic runs must request `--text-workers 1` explicitly, while replay summary v3 continues to
  report the actual selected worker count.
- The shared-memory/deferred-verification recorder revision was built through the cargo-dist path and
  installed hash-first on `infinitas.lan` at `/home/atty/.local/bin/scorepeek`; local, transferred,
  and installed executable SHA-256 are all
  `9be36525987e9565e30a41fe20f02763037778f25602d12115d2c267bfa09057`. The retained target session
  recorded 4,614 ticks and 1,061 canonical frames with zero frame-admission drops, zero field busy
  skips, complete publication, and a 93,457,008-byte recorder memory high-water mark. `doctor`
  reports the
  fixed-slot numeric model active with manifest
  `5e5b545d57a6197f4aaa6a863595f237cb19095903baa135960e4c257cda2137`. No scorepeek process was
  running before or after replacement, so no stale process required restart and no
  `/proc/<pid>/exe` digest was available to compare. This session verifies target recording,
  recording-ready publication, and import while the watcher remains active.
- Public `/v1.sock` authority, target support, prospective target behavior, push, release, and model
  publication remain unverified boundaries.
- The target already has the private numeric manifest bound to the current play-option layout. The
  developer machine's normal numeric store remains on the previous manifest; corpus verification
  used an isolated temporary activation of the current registered manifest without mutating that
  normal store.

## Next executable task

First widen the private label schema and validator beyond the current SP-only slice. After explicit
approval to install, record and label at least one DP RESULT on the target and replay it through
layout v4 to verify the reserved-width ROI and exact DP OCR. Then run the same active v2
generation with one worker and the default pool on the 24-logical-CPU target. Require identical
domain events, less than 20 seconds whole-corpus wall time, and the four-session scaling gates before
claiming target speedup. Continue rebuilding the v2 corpus with session-disjoint songs and failure
attempts; require zero wrong joint acceptance, zero wrong events, and zero missing expected events
before changing target or public-socket authority. Push and release remain separate explicit
boundaries.
