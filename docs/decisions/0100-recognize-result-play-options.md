# ADR 0100: Recognize ordered play options from the RESULT panel

## Status

Accepted

## Context

ADR 0083 deferred play options until real RESULT images covered more than the ordinary RANDOM
display. The current private suite now contains no-option, R-RANDOM, S-RANDOM, MIRROR, A-SCR,
LEGACY, and ordered `RANDOM,LEGACY` examples. Measuring the whole label panel, rather than a suffix
near the graph, is necessary: suffix crops produced fragments such as `RROR` and `GACY` and could
not preserve the complete ordered display.

Play options describe the accepted play attempt but are not required to identify the song/chart or
validate numeric performance. A transient OCR failure therefore must not suppress an otherwise
accepted result.

## Decision

The canonical RESULT layout owns one `x=30, y=318, width=530, height=50` play-option panel. The
registered PP-OCRv6-small text runtime reads the whole panel as a sixth RESULT text job. The first
120 horizontal pixels are also evaluated by a fixed orange marker predicate: a pixel is active when
`r >= 180`, `55 <= g <= 190`, `b <= 90`, and `r >= g + 50`; at least 1000 active pixels means the
label is present, at most 100 means it is absent, and the range between them is inconclusive.

The closed vocabulary is RANDOM, R-RANDOM, S-RANDOM, MIRROR, A-SCR, and LEGACY. A display is
`USE OPTION ` followed by an ordered, comma-separated sequence of distinct vocabulary tokens.
Every such finite sequence is a parser candidate. After trimming and ASCII case folding, the raw
observation is accepted only when exactly one complete display candidate has minimum Levenshtein
distance at most one. Distance two or greater and tied nearest candidates remain typed unknown; no
token-wise recovery or song-specific substitution is performed. A positively inactive marker with
empty OCR becomes a known empty list.

One semantic RESULT episode accepts the ordered list after two matching typed observations.
Conflicting lists and incomplete evidence become `unknown(reason)`. The public
`scorepeek-result-detected-v2` payload gains `play_options`, represented as `known` with an ordered
enum list (including an empty list) or `unknown(reason)`. This optional state never blocks result
finalization or event emission. Raw OCR, marker count, nearest display, edit distance, and temporal
state remain debug evidence only.

Recognition artifacts advance to v16. Debug run events and the observation socket/snapshot advance
to v6. Readers retain recognition v5 through v15 and run-event v2 through v5. Future private
regression labels advance to v5 and require ordered play-option truth; immutable v2 through v4
labels and the current active suite remain readable and unchanged.

Adding the panel changes the canonical-layout digest without changing any numeric cell coordinates
or model bytes. The registered fixed-cell numeric manifest is therefore reissued against the new
canonical and numeric-character-layout digests. Activation in a private store remains a separate
install boundary.

## Consequences

Latest domain and the Resolver TUI can distinguish an accepted empty list, an accepted ordered
multi-option list, and unresolved OCR without deriving domain values from raw text. The sixth text
job may increase RESULT field-worker wall time and busy skips, so prospective target timing remains
required before claiming target-live support.

This supersedes ADR 0083 only for deferring `play_options`, and supersedes ADR 0095 only for the
five-job RESULT text-worker bound. Numeric authority, result identity, attempt linkage, and the
RESULT-close finalization contract do not change.
