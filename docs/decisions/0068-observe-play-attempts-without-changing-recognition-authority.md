# ADR 0068: Observe play attempts without changing recognition authority

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0024 only for excluding application-level provisional play-attempt composition
- Complements: ADR 0058's observation channel, ADR 0059's result presentation, and ADR 0067's
  music-select presentation

## Context

Music selection and result currently stabilize the same catalog identity independently, but the
ordinary TUI does not show whether they belong to one observed play. The intervening song-decision
splash and gameplay cabinet are both measurable layouts. They were previously reported as
`unknown`, even though the splash presents the selected song prominently and the opening gameplay
frames present title and artist again.

The retained 10 Hz target run contains a reviewed sequence of music select, loading and white/black
transitions, two complete decision-splash QOIs, and five complete gameplay-layout QOIs. Replaying
the production predicate classifies the two splash frames as `decide_transition`, the five complete
cabinet frames as `play`, three selection frames as `music_select`, and the six loading, white,
black, or incomplete-cabinet frames as `unknown`. An older one-hertz full-session sample contributes
six further complete cabinet positives plus result, loading, stage-failed, and partial-cabinet hard
negatives. This is sufficient for a provisional path observer, not for accepted event authority or
screen-support qualification.

ADR 0024 correctly removed attempt ownership from the recognition core because attempt inference
must not rescue or modify song resolution. It was too broad for an application presentation that
keeps observed screen facts and their uncertainty visible without feeding them back into recognition.

## Decision

Add independent fail-closed screen predicates for `decide_transition` and `play` in a separately
digest-bound canonical-coordinate screen-path layout. Keep the existing canonical field layout and
machine-profile binding unchanged because normalization geometry and field crops do not change. The
decision predicate requires the measured central full-screen cyan, bright, and saturated
splash anchors. The play predicate requires both the measured left lane edge and right header
anchors. Result, music-select, decision, and play predicates are evaluated independently; exactly one
must pass. Loading, blank transitions, stage-failed presentation, partial cabinet entry, no match,
or multiple matches remain `unknown`. These screens route no OCR crops in this slice. Their bounded
predicate counts are recorded with the existing diagnostic screen facts.

The application owns a synchronous deterministic play-attempt reducer after screen-local temporal
presentation:

- only `stable` and `held_unknown` music-select presentations can arm a selection handoff, and their
  confidence sources remain distinct;
- `decide_transition`, `play`, and `result` advance one session-local monotonic attempt ID while
  preserving explicit path booleans and missing-step reasons;
- a stable result matching the selected ID confirms the attempt; a different stable result retains
  both presentations as a conflict; an isolated result remains unlinked;
- returning to selection or ending the session abandons only an incomplete attempt;
- result-to-play creates a new retry attempt linked by `parent_attempt_id` and inherits the result
  song when available, otherwise the prior selected song;
- unknown frames preserve state, repeated phase observations are idempotent, and course progress is
  not inferred.

`play_attempt_changed` is an additive provisional `scorepeek-run-event-v2` record. Raw
`screen_changed` precedes a path change, and raw result observation plus
`temporal_result_changed` precede confirmation or conflict. The current snapshot retains the same
latest attempt state. The TUI presents it in a dedicated panel; redirected stdout remains limited to
human watcher/session/channel state. No attempt state enters the resolver, accepted `/v1.sock`
events, persistence, score handling, or process exit status.

## Consequences

- The operator can see the selected catalog song remain attached to the observed play and compare it
  with the result without treating raw OCR as identity.
- Missing decide/play screens, an unlinked result, and a conflicting result remain visible instead of
  being repaired by inference.
- Retry attempts are distinguishable without claiming session history, mode progression, course
  structure, or deduplicated domain events.
- OCR of the large title and artist shown during decision and play remains a later corroboration
  source.
