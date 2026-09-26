# Skin specification template

**Responsibility:** Define the headings and evidence to record for one skin's
world, materials, typography, motion, final measurements, justified baseline
adjustments and verified range. It does not add common design rules or API
requirements.

Use these headings for each authored skin. Fill them from the selected concept
and verified implementation, rather than treating the template as evidence.

## Identity and world

Record skin ID, display name and intended world. Describe the world-defining
traits chosen for this skin. Before implementation, state what those traits
must communicate through the frame, information surface, fixed lettering,
dynamic lettering and exceptional states. These are observable cues, not
fixed mock coordinates or prescribed production techniques.

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
Give the skin's color roles and treatments of long or unknown text. For a
material-rich world, link the actual-size ordinary-number and fixed-label
technique comparison across SCORE digits, fixed/judgment labels and History
numbers/headers. State which material cue is visible in the ordinary glyphs
themselves in both hosts, and record that this prototype passed before all
widgets were expanded. Include the native control appropriate to the technique
(neutral font for glyph geometry, undecorated same font for face/edge effects)
and the corresponding browser comparison. If one candidate blurs and another remains
plain, record a revised technique and rerender rather than selecting a relative
winner.

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
For an interval, include minimum, maximum, interior and responsive-breakpoint
samples with the stressed content used at each point. If only discrete points
were rendered, list them as such and mark the interval unverified.
Include frame width, long text, numeric extremes, absent/unknown, difficulty,
clear, rank, EMPTY title/opacity/aspect, and background states. Mark untested
conditions explicitly; do not advertise them as verified.
For each long title, artist or PLAY OPTIONS string used to certify a size,
record its complete visible readback. A full DOM value with a clipped or
ellipsis-marked painted tail is not a verified full-information display.

## Objective checks and visual review

Record ZIP/manifest/resource/state/field/preview checks and their commands.
For final evidence, state the source revision or content hashes used to build
the ZIP and the time/hash of the preview and native/browser renders. Rerun
them after the last Wasm, CSS, font or asset edit; older captures are trial
evidence, not proof of the packaged source.
Link native and browser images or video with content, dimensions and
timestamps. For each widget, link information-filled renders at every
lower-bound size pair, including incomparable narrow and short cases; include
normal-scale views of fixed labels, primary values, the frame, the declared
default size and motion time samples. Add a package-preview-size check for
Selection, Score and History Graph. Link a background-off image of the
information-bearing widgets so their world is judged without canvas art.
For every claimed continuous effect, identify its pixel region and low/high
phase timestamps; record native and browser pixel differences there and confirm
the change is visible at actual size. Record that data regions remain still.
For the exceptional-state gate, include the native and browser History images
from review cases 02 (AAA) and 08 (FULL COMBO); the composite review video
shows only one History case.
For each applicable
[finish gate](basic-design-system.md#self-assessed-finish-gates), record a
pass/fail observation from these images, naming the visible cue or defect
rather than writing only "visually inspected". For each
failure, record the production change and the rerendered result. Record the
unscaled small-type readback and any mismatch before consulting the scene.
Note exceptions and remaining limits. A passing DOM rectangle is not visual
evidence. An existing skin comparison is needed only when a separate
evaluation task requests it.
