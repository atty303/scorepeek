# Skin specification template

**Responsibility:** Define the headings for one skin's current world, materials,
typography, motion, final measurements, justified baseline adjustments and
recommended dimensions. It does not add common design rules or API requirements.
Record run-specific checks and evidence using the
[verification template](skin-verification-template.md) in
`skins/<slug>/verification/verification.md`, not in `SPEC.md`.

Use these headings for each authored skin. Fill them from the selected concept
and verified implementation. Start the skin specification with its title and a
**Responsibility** sentence:
it owns this skin's world, assets, expression, final dimensions, justified
baseline changes; reusable meaning/layout stays in the basic design system and
package acceptance stays in the API specification.

## Identity and world

Record skin ID, directory slug, display name and intended world. Describe the
world-defining traits chosen for this skin. Before implementation, state what those traits
must communicate through the frame, information surface, fixed lettering,
dynamic lettering and exceptional states. These are observable cues, not
fixed mock coordinates or prescribed production techniques.

## Materials and construction

Describe contours, frame segmentation, surfaces, joints, background, original
assets, source/generation method, independently obtained font provenance,
licenses and reproduction steps where applicable.
Trace each defining material cue from the chosen concept to a visible region
of the final information-bearing face in Selection, Score and History. Name the
replacement if a readability fix removed the original cue. An edge treatment
or canvas texture alone cannot prove a glass, metal, print or fabric face.
If the concept has a recognizable canvas field motif, state whether that motif
or a comparably clear replacement remains visible over the package preview's
actual underlay; transparency by itself is not a failure. Give hashes for
skin-owned packaged art. Record each widget's principal regions, proportions,
alignment and spacing. Record
frame-width variants only when this skin actually offers them.

## Typography and information hierarchy

Record fonts, numbers/labels/live text, baselines, label-versus-value color
roles, contrast and how each widget keeps the
[shared information contract](basic-design-system.md#shared-information-contract).
Give the skin's color roles and treatments of long or unknown text. For a
material-rich world, describe the selected lettering technique and at least
two concrete glyph-face or contour features visible at actual size in both
hosts.

## Meaning-specific presentation

Record difficulty colors; the A-to-AA-to-AAA progression in both Score and
History, including the positive cue that distinguishes AA from A; AAA and FULL COMBO treatments; clear, judgment,
FAST/SLOW, missing-value, SYSTEM/RESULT/SELECT status and graph rules. In Score,
name the visible cue that makes PGREAT more positive than GREAT/GOOD beyond its
letters. Record the manifest and painted behavior of no-background and static
canvas modes, plus animated when the canvas concept uses it. List expressive exceptions
and a reason for each. No exception may omit a required item or make it
unreadable at the recommended sizes.
State how score progress, rank distance and unknown MISS RATE graph samples
appear. The supplied score ratio is required for each graph point.

## Motion

Record continuous and state-specific motion, duration/phase, behavior with
motion disabled, and what remains still.

## Baseline adjustments and reasons

For each widget, record the baseline structure and measurements, final
structure and measurements, and the constraint preserved by the adjustment.
State explicitly which baseline measurements remain provisional. If a series
specification applies, link it and separate its adjustments from this skin's
choices.

## Recommended dimensions and states

For status, selection, score, history-list, history-graph and empty, list the
recommended width and height at which all required information is cleanly
painted in both hosts. List separate recommended dimensions for content modes
that need them, such as a 50-row History. Include frame width, long text,
numeric extremes, absent/unknown, difficulty, clear, rank, EMPTY
title/opacity/aspect, and background states. Include recommended heights for
History 5/10/20/50 rows, Graph 1/3/6/12 months, and EMPTY title/no-title,
opacity 0/0.5, wide/tall aspects. An arbitrary resize range is not required.
State current content-length limits and other active constraints.
