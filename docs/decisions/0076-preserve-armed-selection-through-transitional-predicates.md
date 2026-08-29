# ADR 0076: Preserve armed selection through transitional predicates

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0071 only for treating one raw music-select predicate as proof that an armed
  selection was cleared

## Context

A complete target diagnostic recorded the same failure twice after the operator visibly confirmed
an `armed` selection before deciding. In the first path, song `lowercase lifetime` armed at
recognition sequence 888. The decision animation then produced unknown predicates at sequences
907--910, two frames at 911 and 913 that passed the independent music-select color anchors while
all three OCR fields were empty, and the first complete `decide_transition` predicate at 916. The
raw music-select screen change at 911 cleared the selection handoff, so the decision started with
`no_stable_selection`. The second play repeated the same ordering at sequences 1090, 1121, and
1127.

The screen predicates are deliberately independent and frame-local. A transition frame passing
the music-select anchors therefore does not establish that the operator returned to selection or
that the previously stable catalog identity changed.

## Decision

An `armed` selection is a causal handoff to the following decision and survives unknown frames and
raw music-select predicate re-entry. A later temporal music-select update remains authoritative for
replacing or clearing that handoff: a different pending candidate or explicitly empty state clears
it, while the same pending candidate preserves it and a new stable or held identity arms the
corresponding song. A raw music-select screen can still abandon
an already decided or playing incomplete attempt because that state has crossed the handoff
boundary.

No screen predicate, OCR threshold, temporal dwell, recognition authority, or public event schema
changes. The existing ordered diagnostic events remain sufficient to distinguish the raw predicate
from the temporal selection state that owns the handoff.

## Consequences

- The observed `armed -> unknown -> music_select(empty OCR) -> unknown -> decide` path consumes the
  original selected song instead of starting an unlinked attempt.
- A real selection change continues to clear or replace the armed handoff through the existing
  temporal music-select reducer.
- A completed attempt returning to result or selection retains its existing history behavior, and
  a decided or playing attempt that returns to selection remains explicitly abandoned.
