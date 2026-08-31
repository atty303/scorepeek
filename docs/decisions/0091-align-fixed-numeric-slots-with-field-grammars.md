# ADR 0091: Align fixed numeric slots with field grammars

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0090 only for the three-slot miss, FAST, and SLOW character layouts and the
  narrow HYPER tens slot; ADR 0086 and ADR 0087 runtime authority remain unchanged

## Context

The specialist numeric grammar already permits four display digits for every field except level
and combo break. The first fixed-character layout nevertheless provided only three cells for
previous miss count, current miss count, FAST, and SLOW. Their canonical crop ROIs also omitted the
leftmost fourth slot, so a future character recognizer could not recover a four-digit display even
though the typed grammar admitted it.

The measured two-digit HYPER level layout isolated the narrow glyph for the tens digit `1` in a
9-pixel cell while using a 19-pixel ones cell. Level is bounded to 12, but glyph shape does not need
to determine the classifier's input geometry. Equal-width fixed slots give both positions the same
normalization contract while retaining the observed right-aligned `1` inside its slot.

## Decision

- Advance the fixed character artifact to `scorepeek-result-numeric-character-layout-v2`.
- Previous and current miss count, FAST, and SLOW each have four fixed cells ordered from most to
  least significant. Combo break remains three cells and level remains at most two.
- Widen the owning canonical ROIs only to contain those cells: previous miss count becomes
  `(205,595,150,55)`, current miss count `(360,595,150,55)`, FAST `(105,970,80,30)`, and SLOW
  `(105,995,80,30)`.
- The two HYPER level slots both use width 19. The tens slot extends left to `x=840`; the owning
  level ROI becomes `(840,1032,70,32)`. The observed narrow `1` stays right-aligned in that fixed
  slot. No component detection or runtime slot shifting is introduced.
- Re-author the create-only private full-field source dataset from the active reviewed suite with
  the widened canonical crops. Older source manifests remain immutable and are not authority for
  the v2 character layout.
- The current CTC model, calibration, typed resolver, domain event, and TUI paths remain unchanged,
  but the existing full-field observer receives the widened canonical crops for these five fields.
  Its bounded recognition artifact already binds the canonical layout digest and retains numeric
  candidates and decisions, so no parallel telemetry path is added. The new character cells become
  recognition authority only in a later character-classifier checkpoint.

## Evidence

- The active `NumericField` grammar returns four as `maximum_digits()` for both miss fields, FAST,
  and SLOW, while level returns two and combo break three.
- Layout validation requires every v2 cell and marker ROI to be contained in the new canonical
  owner, ordered, non-overlapping, and equal to the expected field cell count. The two HYPER cells
  are additionally checked to have equal width.
- Private v3 source manifest
  `4a1f4f0a513a112732a394cebfe68458a75b9d3041b65fba5fccadbd003488e8` contains 678 re-authored
  crops from seven
  sessions and twenty-seven episodes under canonical layout
  `a20cf2489b580c2b014bb1fb6a3792ebdad4b36bd4646a705e59cdb9a28ad4d4`.
  Truth and provenance are identical to v2; only level, both miss fields, FAST, and SLOW acquire
  new crop ROIs and digests.

## Consequences

- A later fixed-cell classifier can observe four-digit miss and timing values without changing the
  already-approved numeric grammar.
- Equal HYPER level slot widths simplify character normalization without claiming that the glyph
  `1` fills the complete cell.
- The canonical layout digest changes, so new diagnostics and crop artifacts bind the widened
  regions. Historical artifacts retain their recorded layout digest and are not reinterpreted.
- NORMAL, LEGGENDARIA, and unmeasured two-digit BEGINNER or ANOTHER level variants remain
  unsupported and fail closed.
