# ADR 0093: Accept unique exact result titles

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0038's uniform result-title edit-margin requirement

## Context

ADR 0038 required a runner-up title edit margin of at least two for every result title. A retained
target result for the one-character song `A` showed that this rejects valid exact evidence: OCR
repeatedly produced exact title `A` and exact artist `D.J.Amuro`, while the same-artist song `X`
was the runner-up at title distance one. The selected candidate therefore had distance zero but a
margin of only one. Difficulty and notes matched multiple catalog songs and observed level was
unknown, so chart assistance correctly could not replace the title decision.

## Decision

Result resolver v3 distinguishes unique exact evidence from fuzzy evidence:

- a selected title at edit distance zero requires runner-up margin at least one;
- a selected title at nonzero edit distance retains runner-up margin at least two;
- a second exact candidate therefore remains ambiguous;
- chart assistance cannot override a duplicate-exact ambiguity;
- title distance/similarity and artist corroboration gates remain unchanged.

The resolver remains independent of play-attempt state. Numeric truth and a prior music-select
decision do not manufacture a result-song decision, and chart evidence cannot override a
duplicate-exact ambiguity. Existing chart assistance for fuzzy-margin or artist-corroboration
unknowns remains unchanged. Recognition artifacts retain the same candidate metrics and typed
unknown reasons; only the resolver identity advances to
`scorepeek-result-song-title-primary-artist-corroborated-v3`.

## Consequences

- A unique exact short title can confirm the selected play attempt and unblock the existing result
  event path without relaxing fuzzy OCR acceptance.
- Duplicate exact catalog evidence still fails closed with `title_edit_margin_too_small`.
- Existing recognition artifact schemas remain readable and no public domain-event payload changes.
