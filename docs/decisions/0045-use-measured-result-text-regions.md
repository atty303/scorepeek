# ADR 0045: Use measured result text regions

## Status

Accepted

## Context

An ordinary 1920x1080 Wayland foreground session retained three result episodes for the exact songs
confirmed by the operator. The production observer read only the middle of the two long artist
names because the committed result artist region `x=850, y=990, width=220, height=35` excluded both
ends. The short `youhei shimizu` artist happened to fit and decoded exactly.

The result title region `x=660, y=900, width=600, height=100` contained every title horizontally but
also included about half a crop of blank space above the glyphs. The registered dynamic
preprocessor therefore reduced every crop to its 320-pixel minimum input width. This was a sampling
error rather than evidence for changing the OCR model or catalog matcher.

An offline counterfactual used the exact retained canonical QOI for each episode and the same
registered PP-OCRv6-small runtime. Changing only the title region to
`x=660, y=950, width=600, height=50` increased the model input width from 320 to 576 and decoded
`LIGHTNING STRIKES`, `Voo Doo Bamboleo`, and `quick master (reform version)` exactly. Changing only
the artist region to `x=650, y=990, width=650, height=40` increased its input width from 320 to 780
and decoded `BEMANI Sound Team "HuΣeR"`, `SOUND HOLIC Vs. ZYTOKINE feat. CALEN`, and
`youhei shimizu` exactly.

## Decision

Replace the canonical result title and artist regions with the two measured regions above. Keep the
registered OCR model, dynamic preprocessor, candidate scoring, and acceptance thresholds unchanged.
The integrated-context result artist region must remain exactly equal to the canonical result artist
region, and every dependent layout artifact must bind the revised canonical layout digest.

The complete three-episode recording recognition simulation must continue through the production
post-canonical path under the revised digest. The retained foreground QOIs must reproduce all three
operator-confirmed title and artist strings without another play session.

## Consequences

The result field path observes the complete visible artist string and preserves substantially more
title glyph resolution. This change repairs the three retained foreground failures from existing
bounded evidence; it does not establish title-disjoint accuracy, accepted event authority, target
performance, or capture-profile support.
