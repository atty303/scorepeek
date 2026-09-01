# ADR 0097: Finalize evidence-first attempts after semantic results

## Status

Accepted

## Context

Raw screen predicates, asynchronous field OCR, screen-local identity decisions, and play-attempt
state previously shared boundaries indirectly. A transient unknown predicate could end an episode,
an accepted MUSIC SELECT song could remain armed after later evidence moved elsewhere, and RESULT
could emit while additional admitted field work was still pending. Active-title OCR also treated an
empty prefix as exact evidence and could not express the useful distinction between a one-character
foreground rendered as `X` and a long catalog title.

## Decision

The live path is strictly ordered as raw screen classification, semantic screen episode resolution,
screen-local evidence, attempt resolution, and result finalization.

The 10 Hz classifier emits only `known(screen)` or `unknown(reason)`. A semantic episode begins at
the first known screen. Raw unknown suspends rather than closes that episode, and returning to the
same known screen resumes the same episode ID. A different known screen, session boundary, or
reversed chronology closes it. Field work admitted before close remains bound to that episode and
is drained before finalization; work from another generation, after close, or before a chronology
reset is typed late evidence and cannot enter the resolver.

MUSIC SELECT owns selection epochs rather than an accepted song. Evidence intersecting the
incumbent song set accumulates there. Disjoint evidence accumulates in one successor and replaces
the incumbent at support 120. If the screen closes first, the unfinished latest successor snapshot
is handed to the attempt rather than the stale incumbent. A later incumbent observation clears an
unfinished successor. Empty or catalog-common evidence cannot change an epoch.

Attempts own path and evidence snapshots, not separate `selected_song` and `result_song` fields.
Select and RESULT evidence are combined once on the same catalog `(song, play type, difficulty)`
hierarchy. SP and DP charts remain candidates until evidence distinguishes them. Difficulty,
notes, and level only add positive chart support beneath song evidence; level never vetoes.
Missing decide is allowed, while missing select/retry linkage, missing play, unlinked result, and
abandonment remain typed rejections.

RESULT identity, clear type, and numeric performance are provisional while RESULT is displayed.
After its semantic episode closes, every admitted field job is completed or failed and the attempt
is finalized once. A successful finalization records confirmed `play_attempt_changed` before one
`scorepeek-result-detected-v2`. No cancellation event exists because provisional state is debug
only. A direct RESULT-to-PLAY retry inherits the prior selection context once without copying frame
support; observing MUSIC SELECT starts a new context.

All active-list titles use one `TitleEvidenceExtractor`: grayscale greater than 80, the complete
foreground bounding box, four horizontal pixels of margin, and the original ROI height. The
registered PP-OCRv6-small dynamic runtime observes both the wide diagnostic view and foreground
view, but foreground is the lexical authority. Empty and whitespace-normalized observations are
absent and are never compared to the catalog. Raw `X` remains `X`; no song alias or correction is
applied. A nonempty foreground whose width and Unicode scalar count satisfy the registered range
adds a separate structural-length family to non-search catalog title variants of that length.
Lexical, structural, artist, difficulty, notes, and play type therefore resolve identity jointly.

Debug run events advance to `scorepeek-run-event-v3`; raw observations and semantic episode
started, suspended, resumed, closing, and finalized transitions are distinct. The debug socket is
`observations-v3.sock` with a v3 snapshot. Recognition artifacts advance to v14 with title views,
foreground geometry, joint evidence, episode binding, drain status, numeric state, and suppression
evidence. Readers retain run-event v2/v3 and recognition v5 through v14. The public result contract
and `/v1.sock` remain unchanged.

## Consequences

A temporarily unknown screen no longer erases a RESULT or selection. An incomplete final selection
cannot silently hand off an older song. The resolver can combine ambiguous but independent evidence
without requiring perfect OCR, and `〆` may be distinguished from both catalog `X` and a long
same-artist/same-chart collision without rewriting OCR text. Domain latency moves to RESULT close
plus bounded field drain. Target authority still requires a rebuilt reviewed corpus and prospective
sessions with zero wrong events.

This supersedes ADR 0096 for raw-screen episode ownership, current/challenger handoff, and immediate
RESULT authority; ADR 0046 and ADR 0080 for active-prefix-primary title authority; ADR 0059 and ADR
0067 for production result/music-select temporal reducers; and ADR 0071, ADR 0076, and ADR 0077 for
armed-selection ownership. Their historical artifact readers remain supported.
