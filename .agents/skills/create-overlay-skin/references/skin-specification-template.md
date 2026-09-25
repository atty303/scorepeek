# Skin specification template

**Responsibility:** Define the headings and evidence to record for one skin's
world, materials, typography, motion, final measurements, justified baseline
adjustments and verified range. It does not add common design rules or API
requirements.

Use these headings for each authored skin. Fill them from the selected concept
and verified implementation, rather than treating the template as evidence.

## Identity and world

Record skin ID, display name and intended world. Describe the world-defining
traits chosen for this skin.

## Materials and construction

Describe contours, frame segmentation, surfaces, joints, background, original
assets, source/generation method, independently obtained font provenance,
licenses and reproduction steps where applicable.
Link the selected concept and give hashes for skin-owned packaged art. Record
each widget's principal regions, proportions, alignment and spacing. Record
frame-width variants only when this skin actually offers them.

## Typography and information hierarchy

Record fonts, numbers/labels/live text, baselines, label-versus-value color
roles, contrast and how each widget keeps the
[shared information contract](basic-design-system.md#shared-information-contract).
Give the skin's color roles and treatments of long or unknown text.

## Meaning-specific presentation

Record difficulty colors; AAA and FULL COMBO treatments; clear, judgment,
FAST/SLOW, missing-value, SYSTEM/RESULT/SELECT status and graph rules. List expressive exceptions
and a reason for each. No exception may omit a required item or make it
unreadable inside the verified range.
State how score progress, rank distance and unknown MISS RATE graph samples
appear. The supplied score ratio is required for each graph point.

## Motion

Record continuous and state-specific motion, duration/phase, behavior with
motion disabled, and what remains still. Include native and browser evidence
at two or more timestamps.

## Baseline adjustments and reasons

For each widget, record the baseline structure and measurements, final
structure and measurements, the constraint preserved by the adjustment, and
the render evidence. State explicitly which baseline measurements remain
provisional. If a series specification applies, link it and separate its
adjustments from this skin's choices.

## Verified dimensions and states

For status, selection, score, history-list, history-graph and empty, list each
tested width and height or a bounded matrix and the resulting verified range.
Include frame width, long text, numeric extremes, absent/unknown, difficulty,
clear, rank, EMPTY title/opacity/aspect, and background states. Mark untested
conditions explicitly; do not advertise them as verified.

## Objective checks and visual review

Record ZIP/manifest/resource/state/field/preview checks and their commands.
Link native and browser images or video with content, dimensions and
timestamps. Note observed quality, exceptions and remaining limits. A passing
DOM rectangle is not visual evidence. An existing skin comparison is needed
only when a separate evaluation task requests it.
