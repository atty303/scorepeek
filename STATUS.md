# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it. Uncommitted changes are
outside the checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 canonical recognition, evidence-first attempt resolution, and versioned event API: **in
  progress**.
- `scorepeek-result-detected-v2` remains the accepted public domain contract. Debug output uses
  run-event v3, observation socket/snapshot v3, and recognition observation v14.
- Text authority remains the registered PP-OCRv6-small bundle. Numeric authority remains the
  private fixed-cell HOG/MLP model. No model bytes, real crops, complete labels, or generated
  datasets are committed.

## Implemented authority

- The 10 Hz classifier emits raw known/unknown observations independently of field-worker busy
  state. `ScreenEpisodeResolver` owns monotonic semantic episodes. Raw unknown suspends the active
  known screen; the same screen resumes it; only another known screen, session boundary, or reversed
  chronology closes it.
- Field jobs are bound to the semantic episode that admitted them. Episode close stops admission,
  drains already-submitted work, applies it to the closing episode, finalizes RESULT, and only then
  starts the next known-screen attempt transition. Late generation/chronology evidence remains
  diagnostic and cannot enter resolution.
- MUSIC SELECT uses incumbent/successor selection epochs rather than accepted-song arming. An
  unfinished latest successor is handed off on close instead of an older incumbent. Attempt state
  owns screen path and select/result evidence snapshots, not separate selected/result songs.
- MUSIC SELECT and RESULT evidence accumulate on the full catalog `(song, play type, difficulty)`
  hierarchy, including SP and DP. Empty/whitespace observations add no support. Correlated features
  use one family delta per frame; cross-frame raw `u64` sums are normalized proportionally by family
  above the 300 cap. Difficulty, notes, and level provide positive chart support only; level never
  vetoes.
- All active-list titles use the registered foreground extractor: grayscale greater than 80,
  complete foreground bounds, four horizontal pixels of margin, and full ROI height. Foreground
  PP-OCR is lexical authority; wide OCR remains raw diagnostic evidence. Unicode scalar count and
  foreground width add an independent structural family for non-search catalog variants. Raw `X`
  remains `X`; no alias or song-specific correction exists.
- RESULT is provisional while displayed. Only semantic RESULT close after field drain can confirm
  an attempt and emit. Confirmation is recorded before the attempt's sole v2 domain event.
  Unresolved/conflicting identity, missing linkage/play, abandoned attempts, or incomplete required
  numeric tuples complete with typed rejection and emit no result. Direct RESULT-to-PLAY retry
  inherits the parent selection context once without re-adding frame support.
- The TUI retains one three-pane layout. Watcher shows raw and semantic screen plus suspension;
  Latest domain holds only the last accepted v2 event; Resolver shows incumbent/successor/result
  evidence, foreground title geometry, attempt path, and drain/finalize gate. TUI formatting owns no
  resolver logic.
- Run-event v3 distinguishes raw screen observations, semantic episode transitions, selection/result
  and provisional-joint transitions, attempt finalization, and suppression. Recognition observation
  v14 retains title views/geometry, episode binding, fixed-cell numeric evidence, joint support,
  frame timing, late/drain status, and suppression evidence. Readers accept run-event v2/v3 and
  recognition v5 through v14.

## Verification boundary

- Targeted Rust library and binary suites pass after removal of production result/music-select
  temporal reducers. Regression tests require no domain event before RESULT finalization, require
  confirmed attempt before the one event, preserve family ratios, allow accepted state to return to
  conflict before close, and keep direct retry deduplicated.
- Saved target diagnostic `run-1788237882-267982854-944238-session-1` remains outside the corpus and
  was verified read-only as 82 retained canonical frames and 2,190 observations. Current per-frame
  reevaluation observes the `〆` selection as foreground raw `X`, one-character geometry, artist
  `lapix`, ANOTHER, and both SP/DP catalog candidates; RESULT observations retain artist `lapix`,
  SP ANOTHER notes 1877, FAILED, POOR 5, and combo break 5. A resolver regression reproduces the
  catalog `X` and `Flying Castle` collisions and reaches one event only after RESULT finalization.
- The saved diagnostic is not a complete current semantic replay: retained-frame reevaluation does
  not reconstruct the original 10 Hz attempt timeline. Prospective target execution is therefore
  still required for end-to-end authority.
- The existing private corpus and active suite were not changed. The operator plans to rebuild
  them. Title-view/support calibration and wrong-event authority still require a reviewed,
  session-disjoint replacement corpus with zero wrong joint acceptance and zero wrong events.
- Public `/v1.sock` authority, target support, push, release, and model publication remain
  unverified boundaries. Target install/readback for this commit is recorded only after it occurs.

## Next executable task

Run a prospective target session with the installed binary. Verify semantic RESULT close latency,
10 Hz raw cadence, admitted-field drain, foreground-title wall time, busy skips, confirmed attempt
ordering, one event per accepted attempt, and event drop zero. Then rebuild and review private v4
corpus truth before selecting title/support policy authority.
