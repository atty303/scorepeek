# Skin specification template

**Responsibility:** Define the headings and evidence to record for one skin's
world, materials, typography, motion, final measurements, justified baseline
adjustments and recommended dimensions. It does not add common design rules or API
requirements.

Use these headings for each authored skin. Fill them from the selected concept
and verified implementation, rather than treating the template as evidence.
Start the skin specification with its title and a **Responsibility** sentence:
it owns this skin's world, assets, expression, final dimensions, justified
baseline changes and verified evidence; reusable meaning/layout stays in the
basic design system and package acceptance stays in the API specification.

## Identity and world

Record skin ID, directory slug, display name and intended world. For a new
skin, record the distinct-ID check against both the current tree and committed
manifests when old skin files were removed in an isolated checkout. Describe the world-defining
traits chosen for this skin. Before implementation, state what those traits
must communicate through the frame, information surface, fixed lettering,
dynamic lettering and exceptional states. These are observable cues, not
fixed mock coordinates or prescribed production techniques.

## Materials and construction

Describe contours, frame segmentation, surfaces, joints, background, original
assets, source/generation method, independently obtained font provenance,
licenses and reproduction steps where applicable.
Trace each defining material cue from the chosen concept to a visible region
of the final information-bearing face in Selection, Score and History. Name the
replacement and show it at actual size if a readability fix removed the
original cue. An edge treatment or canvas texture alone cannot prove a glass,
metal, print or fabric face. Link final-source native and browser
`background-off` captures that contain those information-filled widgets.
Add a side-by-side material comparison between the selected mock and final
native/browser renders at recommended display size. Identify counterparts for
the information face, an ordinary numeric or fixed-label glyph, and two
separated perimeter spans with their transitions into the face. For each,
name the mock's layering, light and surface cue, the
visible final counterpart and any feature still flattened or missing. Repair
the production material before declaring a pass; matching layout or hue alone
is not equivalent material finish.
If the concept has a recognizable canvas field motif, link a widget-free
region of its final default background and the package preview over its actual
underlay. State whether that motif or a comparably clear replacement remains
visible; transparency by itself is not a failure.
For a physical-material world, link the image-generated frame-joint or
information-face component trial alongside the deterministic candidate and
record which one the actual-size render supports. Record the independent
image-only visual review of the final face, ordinary lettering, full panel
perimeter, Status logo with its surrounding surface, and package preview,
including concrete objections and the rerender that closed
them. A checker's pass or the creator's own favorable assessment does not
close an independent material or legibility failure.
Include the reviewer's unscaled readback of small values, a long History date
and a judgment label, the hardest-to-read glyphs it identified, and its
comparison against an undecorated control with matched size and brightness.
Link the selected concept and give hashes for skin-owned packaged art. Record
each widget's principal regions, proportions, alignment and spacing. Record
frame-width variants only when this skin actually offers them.

## Typography and information hierarchy

Record fonts, numbers/labels/live text, baselines, label-versus-value color
roles, contrast and how each widget keeps the
[shared information contract](basic-design-system.md#shared-information-contract).
Give the skin's color roles and treatments of long or unknown text. For a
material-rich world, link the actual-size live-font, textured atlas or
glyph-mask, and control comparison across SCORE digits, fixed/judgment labels
and History numbers/headers. Record at least two concrete glyph-face or contour
features visible at actual size in both hosts, and why the selected technique
beats the alternatives for this world. Pixel difference alone records rendering,
not material quality. State that this prototype passed before all widgets were
expanded. Match the neutral font control's weight, width and brightness for
glyph-geometry tests; use the undecorated same font for face/edge effects.
If one candidate blurs and another remains plain, record a revised technique
and rerender rather than selecting a relative winner. A clearly flat world may
omit the textured candidate with its reason and actual-size evidence.

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
motion disabled, and what remains still. Include native and browser evidence
at two or more timestamps.

## Baseline adjustments and reasons

For each widget, record the baseline structure and measurements, final
structure and measurements, the constraint preserved by the adjustment, and
the render evidence. Show the information-filled baseline at its stated review
size before changing its geometry. For each change, link native and browser
renders of the baseline and the final geometry with identical content, size
and state; describe whether neighboring labels, values and state cues became
easier or harder to read. A reason without the paired renders does not
establish that the adjusted geometry meets the baseline's purpose. State
explicitly which baseline measurements remain provisional. If a series
specification applies, link it and separate its adjustments from this skin's
choices.

## Recommended dimensions and states

For status, selection, score, history-list, history-graph and empty, list the
recommended width and height at which all required information is cleanly
painted in both hosts. List separate recommended dimensions for content modes
that need them, such as a 50-row History. Include frame width, long text,
numeric extremes, absent/unknown, difficulty, clear, rank, EMPTY
title/opacity/aspect, and background states. Mark untested conditions
explicitly. Additional tested sizes may be reported as individual observations;
an arbitrary resize range is not required.
Record History 5/10/20/50 rows at heights suitable for every row, Graph
1/3/6/12 months, and EMPTY title/no-title, opacity 0/0.5, wide/tall aspects.
Show valid SCORE 0 beside missing/invalid SCORE or NOTES at the same size and
describe how the score-rate bar or dial remains unknown rather than zero.
For each long title, artist or PLAY OPTIONS string used to certify a size,
record its complete visible readback. A full DOM value with a clipped or
ellipsis-marked painted tail is not a verified full-information display.
For Score at its recommended and package-preview sizes, transcribe every visible label, count,
rank distance, bar tick, clear state, timing count and play option solely from
the unscaled native and browser images, then compare those readbacks to the
scene input. Note touching or ambiguous rows even when the text is present in
the DOM; repair and rerender before calling the size verified.

## Objective checks and visual review

Record ZIP/manifest/resource/state/field/preview checks and their commands.
For final evidence, state the source revision or content hashes used to build
the ZIP and the time/hash of the preview and native/browser renders. Rerun
them after the last Wasm, CSS, font or asset edit; older captures are trial
evidence, not proof of the packaged source.
Keep final scene inputs and selected native/browser evidence in a durable
repository location and use relative links from this specification. Temporary
trial captures can explain rejected choices but cannot be the sole final proof.
Link native and browser images or video with content, dimensions and
timestamps. For each widget, link information-filled renders at its
recommended size; include normal-scale views of fixed labels, primary values,
the frame, the declared default size and motion time samples. Add a package-preview-size check for
Selection, Score and History Graph. Link a background-off image of the
information-bearing widgets so their world is judged without canvas art.
For every claimed continuous effect, identify its pixel region and low/high
phase timestamps; record native and browser pixel differences there and confirm
the change is visible at actual size. Record that data regions remain still.
For the exceptional-state gate, include the native and browser History images
from review cases 02 (AAA) and 08 (FULL COMBO); the composite review video
shows only one History case.
Link actual-size native and browser Score/History comparisons from the
controlled `rank-a`/`rank-aa`/`rank-aaa` scenes. Name
the visible positive cue added at AA and the further material cue added at AAA;
different letters alone are insufficient. Record the clear, judgment and timing
meanings separately. Clear types form a progression. PGREAT has a stronger
positive cue than GREAT/GOOD; other judgments have positive and negative
groups. FAST/SLOW are equal-weight directions. A special AAA/FC
badge does not stand in for all semantic states.
Link the final native/browser `background-off`/`background-static` scenes and,
for a moving canvas, `background-animated`. A still canvas records why an
animated canvas control is omitted. Read the material cue on the Select, Score and
History faces with background off; compare background-only regions at low and
high animation phases while the widget data stays still.
For each applicable
[finish gate](basic-design-system.md#self-assessed-finish-gates), record a
pass/fail observation from these images, naming the visible cue or defect
rather than writing only "visually inspected". For each
failure, record the production change and the rerendered result. Record the
unscaled small-type readback and any mismatch before consulting the scene.
Note exceptions and remaining limits. A passing DOM rectangle is not visual
evidence. An existing skin comparison is needed only when a separate
evaluation task requests it.
