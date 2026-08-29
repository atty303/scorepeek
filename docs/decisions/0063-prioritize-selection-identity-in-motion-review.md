# ADR 0063: Prioritize selection identity in music-select motion review

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0061 and ADR 0062 only for leaving the three motion states without a
  deterministic precedence

## Context

The v2 review schema makes `stationary`, `scrolling`, and `selection_change` exclusive states, but
their precedence was not fixed. Ten-hertz review of sequences 987 through 998 shows why the names
alone are insufficient. The active selection changes from `spiral galaxy` to `Sphere` while the
right list also moves; the following frame retains `Sphere` while the list settles. Later pairs
switch from a song to an `ALL VERSION` category and then to another category while their list rows
also move.

Calling every moving-list pair `scrolling` would hide the selection changes that a future dwell
policy must reset. Calling every frame around a selection change `selection_change` would erase the
separate settling interval. A multi-axis schema would retain both facts explicitly, but the draft
already preserves independent list, active-row, and central-title motion evidence. Adding another
review schema before evaluating dwell would duplicate that evidence and increase authoring and
consumer cost.

## Decision

Keep the v2 schema and apply the following precedence to every operator-reviewed adjacent pair:

1. If either bound frame is visibly not the music-selection screen, label the pair
   `screen_context`.
2. Otherwise, if the visible active selection identity differs between the previous and current
   frame, label the pair `selection_change`, whether or not the right list also translates.
3. Otherwise, if the active selection identity is unchanged but the right-list rows visibly
   translate or settle, label the pair `scrolling`.
4. Otherwise, label the pair `stationary` when both the selection identity and right-list row
   placement are unchanged.

The active selection identity is determined from the selected right-list entry and corroborating
central selection presentation where visible. A background, central texture, highlight pulse,
notes-radar animation, or other non-list animation does not by itself make a pair scrolling.
Region-motion measurements may direct visual review but never assign a state.

The precedence is an operator-authoring contract, not a runtime classifier. Raw region evidence
remains unchanged, and review application continues to copy the operator decision without deriving
it. No production predicate, motion threshold, temporal reducer, dwell policy, or event authority
changes.

## Consequences

- Every eligible pair has one reproducible state even when selection change and list translation
  occur together.
- Selection changes take precedence because they are the reset boundary a later dwell evaluator
  must not miss.
- Settling frames after the identity has changed remain separately measurable as scrolling.
- A v3 multi-axis schema is deferred unless the complete reviewed evidence shows that the preserved
  raw motion and this precedence cannot evaluate candidate dwell policies.
