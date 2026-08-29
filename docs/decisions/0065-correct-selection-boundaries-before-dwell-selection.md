# ADR 0065: Correct selection boundaries before dwell selection

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0064's reviewed-set and evaluation artifacts, its claim that every tested dwell
  misses two selection changes, and its treatment of all stability during scrolling as false

## Context

ADR 0063 defines `selection_change` at the adjacent pair where the visible active-selection
identity differs, not at the first animation frame around that change. Reviewing the two reset
misses reported by ADR 0064 exposed two authoring violations. Sequences 2075 and 2076 are identical
frames; the active selection changes at 2077. At sequence 2426, `ABSOLUTE EVIL` still occupies the
selected row and most of the central presentation; the selected row and central presentation change
to `ANEMONE` at 2427.

The earlier decisions therefore placed both reset boundaries one sample too early. This made a
resolver that correctly retained the old identity appear to miss a selection change. It also
showed that the report names `false_stable_nonstationary_pairs` and `false_stabilizations` are too
strong: under ADR 0063, `scrolling` explicitly means list translation or settling while the active
selection identity remains unchanged. Stability during such a pair is an observation, not by
itself an incorrect song decision.

## Decision

Correct the operator review as follows:

- 2075--2076 is `stationary`, 2076--2077 is `selection_change`, and scrolling resumes at
  2077--2078.
- 2425--2426 is `scrolling`, 2426--2427 is `selection_change`, and 2427--2428 remains `scrolling`.

The corrected complete reviewed set contains 713 stationary, 83 scrolling, 30 selection-change,
12 operator-context, and 133 predicate-context pairs. Its SHA-256 is
`e61341576367ee43ada17fcfb78c42f18a0cb4fe60a1cc1fb016c43b429a24a0`.

Version the evaluation report as `scorepeek-private-music-select-dwell-evaluation-v2` and rename
the two activity summaries to `stable_nonstationary_pairs` and
`stabilizations_on_nonstationary_pairs`. Their separate scrolling, selection-change, and context
fields remain structurally separate, but their values are recomputed against the corrected truth.
The rename removes only the unsupported `false` interpretation.

Replaying the same session observations, catalog generation, resolver, and 100/200/300/500 ms
policies against the corrected truth produces zero missed selection-change resets. The policies
reset 4/4, 4/4, 3/3, and 3/3 selection changes that have prior stability. Stationary-run coverage
remains 16/27, 16/27, 13/27, and 13/27. Stable nonstationary pairs are 23/17/16/15, of which
22/17/16/15 are same-identity scrolling; stabilizations entered on nonstationary pairs are 6/1/1/0.
The v2 evaluation SHA-256 is
`0ed18a0f4dd2787e3808f382966d4b30c5e4ece1b957e792fcee0ba3c7048071`.

Select no runtime policy from this motion-only evidence. It now shows that equal-ID dwell resets at
the reviewed identity boundaries, but it still does not label the correct song within stationary
runs or measure whether a policy reduces OCR fluctuation without preserving a wrong accepted ID.
Those correctness labels and a temporal replay comparison are required before runtime selection.

## Consequences

- The two reported reset misses are removed as review-label errors, not resolver fixes.
- Same-identity scrolling stability is retained as neutral evidence and is not called a false
  acceptance.
- ADR 0064's reason for rejecting time-only dwell is invalidated. No dwell is selected yet because
  motion truth alone cannot establish correct-song accuracy or the benefit of temporal smoothing.
- Operator truth remains offline evaluation input only; runtime state and event authority remain
  unchanged.
