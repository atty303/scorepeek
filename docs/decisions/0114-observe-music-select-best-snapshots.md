# ADR 0114: Observe MUSIC SELECT best snapshots separately from results

## Status

Accepted

## Context

RESULT observes an actual attempt, but cannot recover records achieved before scorepeek observed
that play. The left MUSIC SELECT `SCORE DATA` panel exposes the game's current self best. Those
fields can have different achievement dates and options. They must not create result events,
play counts, or inferred plays. DJ rank needs no recognition: it follows from EX SCORE and chart
notes.

## Decision

- Add `scorepeek-music-select-best-layout-v1`, independently measured on scorepeek captures.
  SP and DP use the same panel. Read only SCORE, MISS COUNT, and clear type. Rival/comparison
  values and the rank glyph are outside the crops. The exact `SCORE DATA` header is required.
- Reuse the registered PP-OCRv6-small text runtime and fixed-cell HOG/MLP numeric runtime.
  Four 21x18 cells on a 22-pixel pitch start at `(209,833)` and `(209,861)`. Before the numeric
  model, retain neutral pixels with every channel at least 180 and channel range at most 50;
  dim leading zero placeholders must not be contrast-normalized into digits. Numeric acceptance
  requires a minimum top-two logit margin of 1.0 in every cell and no interior blank. Four measured
  short neutral dashes independently mean no recorded MISS value; empty OCR never means no record.
  Unrecognized clear spellings remain unknown; the observed full-combo label is `FULLCOMBO CLEAR`.
- Model/layout applicability is field-specific. Known, explicit no record, not displayed, and
  unknown are separate typed values. No hidden-panel pattern has been established; absence of the
  required header currently produces unknown, not an inferred not-displayed value.
- The independent `MusicSelectResolver` owns chart identity and field stabilization. Best values
  never contribute identity evidence. A best observation requires a resolved chart, singleton
  current-frame credible song evidence, and matching current-frame known mode/difficulty.
  A chart change or unresolved retreat clears the prior best. Each field needs two consecutive
  equal observations; unknown or a different value restarts only that field. Impossible EX SCORE
  above twice the resolved notes is unknown.
- Admitted input sequence and semantic episode bind the job and output. Duplicate/reversed frames,
  work arriving during suspension, and work older than the resume boundary cannot update best.
  Closing SELECT suppresses supplemental output while existing attempt evidence drains. Raw
  UNKNOWN suspends and retains the pane state; episode end clears it. Revisit is a new observation.
- `MusicSelectBestSnapshot` has its own v1 contract: session/generation, screen episode, selection
  interval, revision, observation ID, source frame sequence and monotonic observation time,
  measured layout identity, resolved chart/presentation, typed fields, and optional derived rank.
  Rank uses integer `floor(9 * score / (2 * notes))` bands and is absent for NO PLAY. Observation
  time is session-relative, not a claimed achievement/play time. No play count, date, option, or
  common-achievement association is inferred.
- Emit `music_select_best_observed` on the first partial/complete snapshot and changed content.
  `music_select_resolver_changed` separately carries current identity status, field streaks,
  activity/suspension, output gate, revision, and current snapshot. Losing all stable values clears
  current availability. Historical notifications remain observations, not corrected play records.
  Both use the existing shared connection and snapshot/replay path. Run-event and socket/snapshot
  become v10; recognition observation becomes v21. RESULT v2 is unchanged.
- With `--record`, raw header/clear OCR, numeric candidates/margins, field failures and episode/frame
  context use the existing diagnostic artifacts. Recording failures cannot change recognition or
  result authority. Without `--record`, no recording is created. No DB is added.
- The TUI has Watcher, Latest result, Music Select Resolver, and RESULT/attempt Resolver panes.
  The new pane orders activity/interval, chart or unresolved reason, best values, then output gate
  and revision. Pending fields show `1/2`; unknown, no record and not displayed have distinct labels.
  Suspension is explicit. The 80x25 layout uses 4/8/7/6 rows and a compact existing resolver view
  that preserves every gate. SELECT-specific current detail moves to the new pane. Formatting
  reads typed resolver state and performs no recognition or stabilization.

## Validation boundary

The private evaluation set includes recorded SP/DP scores, numeric MISS values, full-combo/ex-hard/hard/
normal/failed clears, SP/DP NO PLAY, and transition/other-screen controls. It is outside Git and
has frame digests and complete manual labels. `mise run ocr:select-best:probe` accepts the registered
numeric bundle directory, registered text bundle directory, then canonical QOI paths; both loaders
verify their registered resources. It prints private field observations for comparison to labels.
The conservative numeric gate currently rejects the sampled digit 6; partial snapshots are
intentional. Four-digit MISS, remaining clear types, and target-live performance require
additional captures. Synthetic lifecycle tests cover frame ordering, changed difficulty, suspension,
revisit, partial output, duplicate suppression, and no result-history insertion. Private corpus
replay remains the existing production recognition/result regression gate and reports supplemental
snapshot counts per session. Exact measured coverage belongs in STATUS.md.
