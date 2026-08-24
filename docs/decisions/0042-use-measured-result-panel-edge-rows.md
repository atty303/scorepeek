# ADR 0042: Use measured result-panel edge rows

## Status

Accepted

## Context

The first ordinary Wayland live session retained a canonical result frame but classified it as
unknown. Its result header contained 3,956 warm pixels, while the committed two-row edge crops at
`y=451` and `y=655` produced 457 and 517 pixels above the fixed luma-difference threshold. Lowering
the required count from 518 to 400 would classify that frame, but would also hide a coordinate
error.

An offset scan of the exact retained canonical pixels found 521 and 523 qualifying pixels one row
above, at `y=450` and `y=654`. Independently extracted canonical frames from all three reviewed
2026-08-17 result episodes show the same direction: the revised rows produce 524/526, 524/526, and
525/525-526 on stable frames, while the old rows produce 520-522. This agreement spans the direct
Wayland and separate recording profiles; neither profile is treated as the other's pixel reference.

## Decision

Keep `horizontal_edge_pixels_min=518` and move only the upper and lower two-row result edge crops
from `451/655` to `450/654`. The canonical layout digest changes with those coordinates. Do not
relax the predicate to compensate for a measured layout error.

The exact direct-live QOI must classify as result under the revised committed layout. The complete
recording recognition simulation must still pass both failed results and the success result through
the production post-canonical field, catalog-scoring, clear-type, and song-resolution path.

## Consequences

The direct-live failure can be repaired offline without another play. This decision establishes the
canonical recognition coordinates, not the correctness of the source-to-canonical transform. A
canonical QOI alone cannot independently replay that transform; source evidence is governed by ADR
0043.

