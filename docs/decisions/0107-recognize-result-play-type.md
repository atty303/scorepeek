# ADR 0107: Recognize RESULT play type before chart resolution

## Status

Accepted

## Context

The active private corpus contains all five SP difficulty layouts. Its newest session exposed a
catalog sibling collision: `Wizards!` SP HYPER and DP HYPER have the same level and notes, so title,
artist, difficulty, level, and notes left the attempt at `joint_identity_unresolved`. RESULT chart
resolution still provisionally selected SP in one field-local path, while the attempt resolver did
not treat that default as evidence. Guessing SP would resolve this fixture but would make a future
DP result unsafe.

Across the operator-reviewed SP stable frames, the `SP` label occupies the same slot beside notes
despite the difficulty-specific layout differences. The observed glyph bounds are approximately
`x=934..989, y=1033..1070`. No DP frame is present in the corpus.

## Decision

Integrated context layout v4 adds a RESULT play-type ROI at `(925,1025,75,50)`. Its left margin is
wider than the observed SP glyph bounds to accommodate the wider first glyph in `DP`, while its
right edge stops before the notes field. This is a measured SP ROI and a DP-width provision, not DP
image validation.

The registered PP-OCR runtime observes the crop as a seventh independent RESULT text job under
result-field resolver v6. Parsing
is an exact closed enum after ASCII whitespace trimming: only `SP` and `DP` become typed catalog
play types. Empty, unrecognized, or other text has no chart authority. The field-local RESULT chart
resolver now requires the observed play type instead of supplying SP.

Attempt resolution gives play type its own evidence family. It contributes fixed chart support
only after at least two observations of one play type and no observation of the other play type in
the semantic RESULT episode. Until then, chart identity cannot be accepted. Once known, candidates
of the opposite play type are excluded rather than merely receiving less support. A single
observation or any SP/DP conflict contributes no play-type support. Difficulty, notes, and level
remain a separate correlated chart family.

Recognition observation advances to v19 and retains the raw play-type OCR. Run event, observation
socket, and snapshot advance to v7 so resolver diagnostics can expose the new family and raw field.
The accepted `scorepeek-result-detected-v2` event contract does not change.

## Consequences

Catalog siblings with otherwise identical chart metadata can resolve without a provisional SP
default, and the active two-session corpus must preserve all fourteen accepted attempts. Existing
SP difficulty layouts share the new slot; their other ROIs remain unchanged. DP glyph width is
covered geometrically and the typed parser/resolver path exists, but DP recognition and target
support remain unverified until an operator-reviewed DP capture passes the same gates.

The text pool bound rises from six to seven jobs per RESULT frame. Corpus replay errors retain the
field-observation cause instead of collapsing it to a generic OCR failure.

This supersedes ADR 0078 only for treating SP/DP detection as wholly unimplemented and ADR 0098
only for RESULT play-type evidence. Their public-event, hierarchy, and fail-closed contracts remain
in force.
