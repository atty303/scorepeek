# ADR 0067: Stabilize music-select with bounded hold and replacement

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0066's equal-ID reducer, its fixed single-candidate correctness report, and its
  conclusion that temporal presentation adds only unresolved latency
- Complements: ADR 0059's result-local temporal presentation and ADR 0058's provisional
  observation channel

## Context

ADR 0066 evaluated a reducer that cleared both pending and stable state on every unknown and also
cleared the old stable value as soon as a different accepted ID appeared. That is not the intended
result-like presentation contract. A stable result survives an isolated unknown; music-select
needs the same continuity and must also replace the stable value after a real selection change.

Music-select cannot copy result behavior exactly. A result-local unknown remains within one result
episode, while music-select can legitimately move from a song to a category or filter that resolves
to no song. Unbounded retention would therefore leave a previous song visible indefinitely.
Different accepted values are also normal selection changes rather than terminal conflicts.

The corrected motion truth and complete correct-song truth remain valid. The former contains 713
stationary, 83 scrolling, and 30 selection-change pairs. The latter labels 18 song runs and nine
non-song category/filter runs. Frame-local resolution remains 729 correct, zero incorrect, and 11
unknown over 740 stationary observations, with no accepted-ID transition.

## Decision

Add a deterministic music-select temporal reducer after the frame-local resolver. It has no clock,
filesystem, queue, catalog, or OCR dependency and consumes only source sequence, monotonic time,
and an optional accepted song ID.

- `pending` requires one accepted ID to persist for the configured dwell before initial stability.
- `stable` confirms only a currently matching accepted ID.
- An unknown moves stable state to `held_unknown`. The prior ID and evidence remain available as a
  last-confirmed presentation, but are not a current accepted value.
- A different accepted ID moves stable or held state to `changing`. The prior presentation and new
  candidate remain distinct; neither is a current accepted value while replacement is pending.
- A return to the prior ID cancels the hold or change without reacquisition. A candidate that
  persists for the dwell replaces the prior stable value. A third ID replaces only the candidate.
- Unknown retention expires after a bounded grace. A gap over 250 ms, reversed monotonic time,
  non-music-select screen, session boundary, or watcher boundary resets all state.

Select 200 ms for both dwell and unknown grace. The offline evaluator uses this production reducer,
not a private equivalent, and compares the full 100/200/300/500 ms dwell by 100/200/300 ms grace
matrix. Its versioned v2 report keeps frame-local output, confirmed temporal output, retained and
changing observation counts, transitions, per-run correctness, and non-song final retention
separate.

The canonical report SHA-256 is
`328a6476eafa71c4e79796112088814b318306d8a1037ad5e4c723e1fc05bb38`. The selected 200/200 ms
candidate has 705 confirmed-correct, zero confirmed-incorrect, and 35 unconfirmed stationary
observations, preserves 16/18 song-run coverage, stabilizes no non-song run, and retains no song at
the end of any non-song run. Across all 982 replay observations it records 661 stable, ten
`held_unknown`, and five `changing` states, with no wrong stable streak. Its transitions distinguish
three pending candidates cleared by unknown from five actual unknown-grace expirations. The 100 ms
dwell reaches the same coverage with lower acquisition latency, but ADR 0065's motion result shows
six nonstationary stabilization entries versus one at 200 ms. This justifies the 200 ms dwell as
the bounded robustness tradeoff; the grace affects retained presentation rather than confirmed
correctness.

`scorepeek run` emits `temporal_music_select_changed` only after the raw `field_observation` that
caused it. The provisional record carries the typed reducer state, transition reasons, retained
catalog presentation, and pending candidate presentation. The client snapshot and TUI derive from
the same event reducer. TUI labels retained unknown state as `HELD` and replacement as `CHANGING`;
it never labels the old presentation as current accepted. Raw active and central OCR remain a
separate observation surface and are not rewritten.

The event uses the existing bounded observation socket and inherits its sequencing, health,
non-interference, and recording opt-out contracts. It remains provisional presentation, not the
future accepted event API or event authority.

## Consequences

- One-frame resolver unknowns no longer remove the last-confirmed catalog title and artist from the
  TUI, while raw OCR variation stays visible as evidence.
- A real song change becomes observable immediately and replaces the old stable value only after
  200 ms of consistent accepted evidence.
- Category and filter selections can retain a visibly labelled last-confirmed song for less than
  the bounded grace, then clear it; they cannot leave an accepted or indefinite stale song.
- The historical ADR 0066 artifact remains evidence for the rejected clear-on-unknown reducer but
  is not the runtime policy evaluation.
