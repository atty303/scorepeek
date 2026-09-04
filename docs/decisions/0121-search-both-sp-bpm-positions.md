# ADR 0121: Search both SP BPM positions

## Status

Accepted. Supersedes ADR 0113's fixed-position premise and search region only.

## Context

A retained canonical target capture shows that moving the SP graph beside the note lane also
moves the BPM panel right. Independently measured outlines start at `(866,952)` with the graph
on the right and `(1283,952)` with the graph on the left. They measure `340x71` / `339x71` and
contain 5,972 / 5,947 cyan pixels. Both satisfy the existing hollow-outline predicate, but the
second lies outside layout v3's `(760,940,480,140)` search. Missing PLAY prevents an otherwise
observed RESULT from becoming a confirmed play when no PLAY tick was accepted.

## Decision

- Screen-path layout v4 searches `(760,940,900,140)`, covering both measured positions with one
  bounded component pass. This preserves the original region and adds the right-hand position;
  no graph-setting inference or alternate recognition model is needed.
- Keep every color, component size and hollow-row threshold from ADR 0113, and keep the
  exactly-one-peer-screen rule. The wider search does not make cyan area alone sufficient.
- Retain existing raw predicate geometry diagnostics and layout digest binding. The public event
  API and result acceptance gates are unchanged. A matching component remains preferred over a
  larger nonmatching component.
- The search area is 126,000 pixels, below the 128,000-pixel loader bound. No frame-sized search,
  new dependency, persisted state or diagnostic protocol is introduced.

## Consequences

Both measured SP placements can be recognized. Existing SP/DP and non-PLAY controls must remain
part of verification because more of the lower screen is inspected. Retained-frame evaluation
is evidence for these recorded layouts, not a new target-live performance or capture gate.
