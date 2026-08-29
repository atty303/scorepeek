# ADR 0066: Evaluate the leading music-select dwell with correct-song truth

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0065 only for leaving every dwell candidate equally unranked and for requiring
  correct-song evaluation as future work

## Context

The corrected motion evaluation gives 100 ms and 200 ms equal stationary-run coverage (16/27) and
selection-change resets (4/4). The 200 ms candidate enters stability on only one nonstationary pair,
compared with six at 100 ms, while 300 ms and 500 ms reduce coverage to 13/27. This makes 200 ms the
leading motion candidate, but motion truth cannot distinguish a correct song, a wrong song, and a
stationary category or filter selection.

The visible central title also animates and its OCR text can vary between frames. Correct-song
evaluation must distinguish that raw OCR variation from accepted song-ID variation; otherwise a
dwell may appear beneficial even when the production resolver already rejects the varying text.

## Decision

Add the create-only offline command:

```text
scorepeek-corpus music-select dwell evaluate-correctness --store ROOT --catalog-store ROOT --reviewed REVIEWED --labels LABELS --output REPORT
```

The canonical `scorepeek-private-music-select-correct-song-labels-v1` document binds the corrected
reviewed-set SHA-256 and assigns exactly one ordered label to every maximal stationary run. A label
is either a catalog-bound `song` with `scorepeek_song_id`, or `not_song_selection` for a visibly
stationary category or filter. Missing, reordered, partial, non-catalog, or non-canonical truth
fails closed. Operator labels remain evaluation-only and never enter the resolver or dwell state.

The evaluator replays the complete span through the same equal-accepted-ID reducer as the motion
evaluation, with 200 ms fixed as `leading_motion_candidate`. Its
`scorepeek-private-music-select-correctness-evaluation-v1` report compares frame-local and stable
outputs over the labeled stationary observations. It records correct, incorrect, and unknown
counts; accepted-ID and outcome transitions; song-run coverage; category/filter output; correct
stabilization latency; and duration of wrong stable streaks. It includes per-run results, sets
`runtime_policy_selected=false`, and grants no event authority.
An expected song match and no output for `not_song_selection` are correct; no output for an
expected song is unknown; every other song output is incorrect.

The reviewed video contains 27 stationary runs: 18 visible song selections and nine category or
filter selections. Across 740 observations the frame-local resolver produces 729 correct, zero
incorrect, and 11 unknown outputs, with zero accepted-ID transitions and two correct/unknown
transitions. It resolves 16/18 song runs and produces no song on any of the nine non-song runs.

The 200 ms candidate produces 705 correct, zero incorrect, and 35 unknown outputs. It also resolves
16/18 song runs, produces no song on non-song runs, and never stabilizes a wrong ID. Correct
stabilization is p50 200 ms, p95 300 ms, and maximum 300 ms. The label SHA-256 is
`ad9e2e0c8ea4b1d90a303d0e70c5fb1dd74b64c0dbac186673ad04f868bd7299`; the evaluation SHA-256 is
`53f7afe0cb548f5c847baa53cc333e2fbf9f25353f5739b323198c7b9789f23b`.

Keep 200 ms as the leading offline candidate, but do not implement it in the runtime. This corpus
shows no accepted song-ID jitter for dwell to remove and shows 24 additional unresolved
observations from stabilization latency. Raw central-title text variation remains a separate
presentation/evidence question.

## Consequences

- The current evidence establishes that 200 ms does not retain a wrong song or turn a stationary
  category into a song in this bounded recording.
- It does not establish an accuracy gain over frame-local resolution; accepted song identity is
  already stable wherever it is present.
- Stabilizing song IDs would not by itself stabilize the raw `central_title` OCR line shown by the
  TUI. That text must be measured separately before changing presentation.
- Runtime music-select temporal state, stable-selection events, and event authority remain
  unimplemented.
