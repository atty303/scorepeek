# ADR 0117: Recognize the SELECT marker outline

Status: Accepted

## Context

Stationary SELECT runs retain the same visible PLAYER 01 marker while background animation changes
RGB area counts. Two captured HYPER intervals contain 40 unknowns in 112 observations: 32 fail the
fill minimum, six have a second qualifying background slot, and two fail the winner margin. The
old HYPER crop also includes background to the right of the marker. Lowering its fill threshold
would not address competing background candidates.

## Decision

Use independently measured canonical marker positions in integrated-context layout v6 and the
shared `scorepeek-player-marker-outline-v2` predicate. Confirm the upper and lower thin neutral
bright edges against their non-bright interior neighbors. Score each slot by its weaker edge;
require 80% matching columns on both edges, a single qualifying slot, and a 10 percentage point
winner margin. The right upper segment avoids the marker pointer; interior samples avoid text.
No game bitmap, glyph template, model, new dependency, or upstream resource is embedded.

Preserve per-frame evaluation and the latest-known difficulty semantics of ADR 0099. Unknown
remains explicit for absent, competing, or insufficient-margin evidence. Do not use best values,
historical difficulty voting, or selection persistence to repair raw recognition. Best snapshot
interval/deduplication behavior is a separate change.

Diagnostic observation v22 and run-event/socket/snapshot v11 replace RGB area metrics with the two
edge fractions. Retain historical recording readers. RESULT v2 and best snapshot v1 are unchanged.
The existing frame-bound diagnostic path is sufficient; no additional recording or export is added.

## Verification boundary

Use the production Rust predicate on retained canonical frames, compare with private labels and
recorded baseline observations, and run the accepted RESULT corpus regression. Synthetic cases
exercise every slot, missing/competing markers, broad white bands, and insufficient margin.
Private frames and complete truth stay outside Git. Coverage, remaining capture conditions, and
verified/unverified boundaries are recorded in STATUS.md; development replay is not target-live
validation.

This supersedes only the RGB panel/fill/glyph predicate in ADR 0096.
