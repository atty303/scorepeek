# ADR 0096: Resolve screen episodes before play attempts

## Status

Accepted

## Context

The result and music-selection paths reduced each OCR frame through a screen-local title-primary
decision and then required separately accepted fields to agree. In practice this made independent
imperfect observations behave as an AND gate. The 10 Hz screen predicate already remained active
while field OCR was busy, but screen continuity, identity evidence, attempt linkage, and TUI
presentation did not share one hierarchy.

## Decision

The live path is ordered as screen classification, screen-local evidence accumulation, play-attempt
resolution, and domain acceptance.

Each screen-class change, session boundary, or reversed chronology starts a monotonically numbered
screen episode. Field OCR remains an asynchronous producer of raw text and typed numeric values.
The screen adapter maps full-catalog text metrics and typed chart fields to integer support for
joint `(song, play type, difficulty)` hypotheses. Each candidate keeps an unsigned 64-bit raw sum
per evidence family. At summary time, a family whose largest candidate remains at or below 300 is
unchanged; otherwise every candidate in that family is scaled by the same `300 / maximum` factor.
This bounds a repeated presentation without destroying the relative margin between its candidates.
Empty observations add no support and do not erase prior evidence.

MUSIC SELECT owns current and challenger accumulators. A disjoint challenger replaces current only
after exceeding the fixed change margin. Pixel motion is not a production input. RESULT owns one
accumulator for its whole screen episode. Level contributes only positive support and never vetoes
a candidate. Clear type and numeric performance retain their independent typed validation.

MUSIC SELECT difficulty is not text. Canonical integrated-context layout v3 contains five fixed
`PLAYER 01` marker slots for BEGINNER, NORMAL, HYPER, ANOTHER, and LEGGENDARIA. One shared RGB
panel/fill/glyph predicate yields `known(difficulty)` only when exactly one slot clears the minimum
and winner margin. No marker, multiple markers, and insufficient margin remain typed unknowns.
Only the central title, artist, and active-list title enter PP-OCR. A known marker narrows sibling
charts already reached by song evidence and never creates a song candidate by itself.

The attempt reducer records selection-screen presence even when no song was accepted. A later
accepted joint result may therefore complete a path with an observed selection, play, and result;
PLAY or RESULT without select or retry linkage remains unlinked. Domain promotion still records a
confirmed `play_attempt_changed` before exactly one `scorepeek-result-detected-v2` event. Returning
through MUSIC SELECT starts a fresh selection path even when its identity remains unknown; retry
inheritance is reserved for a direct RESULT-to-PLAY path. Event deduplication is keyed by attempt ID
and therefore survives transient screen-episode breaks.

TTY output has one vertical layout: a four-row Watcher pane, a nine-row Latest domain pane, and a
remaining Resolver pane. The latest-domain pane reads only accepted v2 events. The resolver pane
formats a typed debug snapshot and never recalculates support or gates. A private screen tick updates
integer-second monotonic durations without entering run-event artifacts, observation sockets, plain
output, or domain output.

Recognition observations advance to v13 and retain the typed marker observation, joint evidence,
raw stage timing, and completed/late field status. Typed `resolver_state_changed` run events record
only state, top, runner, or accepted-identity transitions, including reset and candidate switch,
with raw and normalized family contributions. Per-source-sequence diagnostics finalize timing only
after synchronous resolver and output work has completed; unexecuted stages remain absent rather
than measured zero. Busy-skip, not-applicable, failed, and late-episode statuses do not change
resolver semantics. Existing v5 through v12 readers remain accepted.
Private regression labels advance to v4 by adding optional
attempt linkage, sequence spans, parent linkage, and expected outcome; v2 and v3 remain readable.

## Consequences

An exact field can be decisive, while several fuzzy fields and select/result observations can reach
the same threshold together. The accepted event contract remains unchanged. Resolver policy values
are explicit and bounded, and must pass session-disjoint private evaluation with no wrong joint or
domain acceptance before target authority is claimed.

This supersedes ADR 0038 and ADR 0093 for title-primary result authority, ADR 0046 and ADR 0080 for
active-prefix-primary music-selection authority, ADR 0059 and ADR 0074 for identity stabilization,
ADR 0088 and ADR 0089 for the TUI layout, and ADR 0095 only where frame timing is represented by the
new v13 observation and per-frame diagnostic contract.
