# ADR 0090: Fix result digits to canonical character cells

- Status: Accepted
- Date: 2026-08-31
- Extends: ADR 0087's fixed-result numeric recognition boundary
- Supersedes: ADR 0087's development-evidence claims that level and notes crops are
  background-only, absent from the retained `Horizons of Promise` QOI, and excluded from the
  numeric dataset; its runtime and calibration decisions remain in force

## Context

The sequence CTC model consumes each numeric field as one resized image. A bounded character-level
spike instead classified every correctly separated judgment and combo glyph, including the retained
PGREAT and POOR failures. Its thirteen unresolved crops were not unreadable glyphs: nine were lost
by a dynamic gap heuristic, one admitted an unrelated bright component, and three joined a bright
background into the foreground mask. Several of those observations preceded the operator-selected
stable frame while the result values were still sliding into place.

The canonical result presentation does not require image-driven glyph discovery. Stable QOIs show
fixed score, judgment, timing, combo, and notes positions. Notes always displays four digits with a
leading zero when necessary. Level follows the rendered difficulty text, so its cells are fixed by
the already-typed difficulty and displayed digit count rather than by one global x coordinate.

## Decision

- `scorepeek-result-numeric-character-layout-v1` binds the existing canonical layout digest and
  lists every digit cell in canonical-frame coordinates. Ordinary fields list cells from most to
  least significant. A character recognizer may accept only leading blank cells followed by one
  contiguous digit sequence; it does not search for components, shift cells, or infer gaps from
  pixels.
- Current and previous score, all five judgments, notes, miss values, FAST, SLOW, and combo break
  use explicit fixed cells. Notes has four visible cells and retains its displayed leading zero.
  The dash presentation for previous score and miss values remains one field-level
  `not_displayed` marker instead of being forced into two independently segmented dash glyphs.
- Level uses fixed variants selected by `(difficulty, displayed_digits)`. The reviewed QOIs define
  one-digit `HYPER`, `BEGINNER`, and `ANOTHER` plus two-digit `HYPER`. Any unmeasured combination
  stays unsupported and must fail closed; runtime image content cannot select a substitute layout.
- Numeric dataset authoring includes level and notes truth. Because pre-stable result frames can
  contain identical blank chart crops under different chart truth, level and notes are collected
  only from the operator-selected stable QOI. The existing selection policy for the other twelve
  full-field crops is unchanged in this checkpoint.
- The current CTC runtime, model activation, calibration, domain event, and TUI contracts remain
  unchanged. This checkpoint defines and validates the layout and source corpus for the later
  character recognizer; it does not grant character predictions runtime authority.

## Evidence

- Operator correction republishes `current-7-9` COMBO BREAK as `30` in immutable label
  `0cb36469503ef363cef6eea1eab0e8f08955e3b29de25978f79200a9dc3c8c92` and active suite
  `4b0fc906f0a74efb52d406850a3c611f6741d4b49b8228ac2efc8bae56ffe78c` without changing the
  earlier label or suite objects.
- Private dataset v2 manifest
  `e52dd198fac8abd5dc4a67a316903a31e270c5e28aa2ba6f034d21bb6e39e02b` contains 678 unique
  full-field crops from seven sessions and twenty-seven episodes, including 23 level and 26 notes
  crops retained after global cross-session digest exclusion. Notes labels preserve the four
  displayed glyphs, including their leading zero. The earlier v1 manifest remains immutable but is
  not character-training authority because its notes labels omitted that glyph.
- Stable notes components occupy four 21-pixel-pitch cells. Stable level QOIs cover HYPER levels
  6--10, BEGINNER level 1, and ANOTHER level 7. The fixed layout loader rejects a changed canonical
  layout digest, cells outside their owner ROI, reordered or overlapping cells, wrong cell counts,
  and an incomplete or reordered level-variant set.

## Consequences

- The next classifier experiment receives one normalized glyph cell at a time plus explicit blank
  evidence. Dynamic connected-component segmentation is not part of the production design.
- Sliding or otherwise off-layout observations naturally remain unknown rather than being assigned
  stable result truth.
- NORMAL and LEGGENDARIA, plus two-digit BEGINNER and ANOTHER level cells, require reviewed QOI
  evidence before those variants can be added. Their absence does not affect the already measured
  variants or the current CTC runtime.
