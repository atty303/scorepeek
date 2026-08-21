# ADR 0023: Link screen context by play attempt and record replayable live evidence

- Status: Accepted
- Date: 2026-08-21

## Context

The product goal is to avoid missed result records. Dedicated play solely to
collect result screens is costly, while stationary music-select titles provide
cheap text-rendering evidence close to result titles. A large prepared timeline
dataset is therefore an unsuitable prerequisite for developing the session
path. Recognition-triggered diagnostics alone are also insufficient: if screen
detection, OCR, and event emission all miss a result, they hide the denominator.

Both screens expose useful local song context. Result has title, artist, play
mode, difficulty, level, and notes around its central lower identity area.
Music select has the central selected title and artist, play mode, selected
difficulty and level, plus the active title in the right-hand song list. The
detailed score/radar panels at the far left or right add layout dependence and
are not part of the initial identity contract.

## Decision

Resolve song identity independently within each screen context:

- result: title, artist, play mode, difficulty, level, and notes;
- music select: central selected title, artist, play mode, selected difficulty,
  selected level, and the active right-list title.

The central and active-list titles are two presentations of the same selection,
not independent metadata votes. Agreement may corroborate identity; readable
conflict rejects the candidate or marks a transition. The active-list title is
runtime evidence, but it does not enter the provisional non-selected-row
training corpus without its own label and presentation provenance. Exact notes
are not expected on music select.

Maintain two temporal scopes. A screen-local episode stabilizes fields and
deduplicates that screen's events. An explicit `play_attempt` links a stable
music selection through observed gameplay to a result. A linked selection may
corroborate or rescue weak result song identity only when the transition is
unbroken and capture generation, profile, normalizer, canonical layout,
catalog, and model bindings match. It cannot establish result-screen presence,
savability, score, or other result-only fields, and cannot conceal a missed
result episode. Unknown transitions, reconnects, or binding changes reset the
attempt and produce unknown rather than a guess.

Develop the deterministic state machine from a small set of recordings that
cover explicit scenarios; do not require a statistically large hand-prepared
timeline first. Ordinary live use must create bounded, local, replayable
diagnostic evidence at a calibrated sparse cadence independently of recognizer
success. Until the ROIs and screen predicates stabilize, retain lossless
canonical full frames so detection and future ROI changes can be replayed;
later reduction to a versioned padded ROI atlas requires byte-equivalent replay
conformance. It must retain monotonic sequence/timing,
transitions, model/catalog/profile/layout/normalizer bindings, candidate and
decision state, emitted/suppressed/absent outcome, and completeness
(`complete`, `partial`, or `dropped`) to rerun screen and song decisions. Exact
format, cadence, byte/time limits, and retention policy are deferred to the
implementation slice.

A segment may be `complete` for result-miss accounting only when its maximum
observation gap is below the calibrated minimum result dwell. A capture gap,
drop, or unavailable artifact makes the affected interval `partial` or
`dropped`; absence of a result there is not evidence that no result occurred.

Diagnostic recording is observational and must not alter recognition or event
delivery. It is local and bounded by default, can be disabled, and has no remote
export without opt-in. The public NDJSON event API remains free of pixels and
raw OCR candidates. The word `private` denotes repository and redistribution
boundaries, not automatic secrecy: song strings and artifact identifiers need
not be redacted solely for that label. Lossless full frames may incidentally
contain player or rival UI only in the operator-controlled local private
retention needed for replay; they remain excluded from commits, the public API,
and default remote export, and are not extracted as recognition fields.

## Consequences

This refines ADR 0018's result-evidence sequencing: low-cost music-list work is
still valid, but development uses a few scenario recordings and live replayable
telemetry rather than a large prepared timeline. It complements ADR 0016; the
non-selected stationary rows remain transfer evidence, while the active row is
screen-context evidence. ROI coordinates, telemetry schema, state-machine code,
and support thresholds are deliberately not selected by this ADR.
