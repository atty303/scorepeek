# Skin verification template

**Responsibility:** Define the run-specific checks and evidence for one skin.
Create `skins/<slug>/verification/verification.md` from these headings, and keep
its scene inputs, native/browser images, review video and other evidence under
`skins/<slug>/verification/evidence/`. This directory is Git-ignored. Use
relative links from `verification.md` to its evidence. The current skin
specification remains in `skins/<slug>/SPEC.md`.

## Identity and world checks

For a new skin, record the distinct-ID check against both the current tree and
committed manifests when old skin files were removed in an isolated checkout.
Link the selected concept.

## Materials and construction

Link final-source native and browser `background-off` captures that contain
information-filled Selection, Score and History widgets. Show the replacement
at actual size if a readability fix removed an original material cue.
Add a side-by-side material comparison between the selected mock and final
native/browser renders at recommended display size. Identify counterparts for
the information face, an ordinary numeric or fixed-label glyph, and two
separated perimeter spans with their transitions into the face. For each,
name the mock's layering, light and surface cue, the visible final counterpart
and any feature still flattened or missing. Repair the production material
before declaring a pass; matching layout or hue alone is not equivalent
material finish. If the concept has a recognizable canvas field motif, link a
widget-free region of its final default background and the package preview
over its actual underlay.

For a physical-material world, link the image-generated frame-joint or
information-face component trial alongside the deterministic candidate and
record which one the actual-size render supports. Record the independent
image-only visual review of the final face, ordinary lettering, full panel
perimeter, Status logo with its surrounding surface, and package preview,
including concrete objections and the rerender that closed them. A checker's
pass or the creator's own favorable assessment does not close an independent
material or legibility failure. Include the reviewer's unscaled readback of
small values, a long History date and a judgment label, the hardest-to-read
glyphs it identified, and its comparison against an undecorated control with
matched size and brightness.

## Typography and information hierarchy

For a material-rich world, link the actual-size live-font, textured atlas or
glyph-mask, and control comparison across SCORE digits, fixed/judgment labels
and History numbers/headers. Record why the selected technique beats the
alternatives for this world. Pixel difference alone records rendering, not
material quality. State that this prototype passed before all widgets were
expanded. Match the neutral font control's weight, width and brightness for
glyph-geometry tests; use the undecorated same font for face/edge effects.
If one candidate blurs and another remains plain, record a revised technique
and rerender rather than selecting a relative winner. A clearly flat world may
omit the textured candidate with its reason and actual-size evidence.

## Motion

Include native and browser evidence at two or more timestamps.

## Baseline adjustments and reasons

For each changed widget, record the render evidence. Show the
information-filled baseline at its stated review size before changing its
geometry. Link native and browser renders of the baseline and the final
geometry with identical content, size and state; describe whether neighboring
labels, values and state cues became easier or harder to read. A reason without
the paired renders does not establish that the adjusted geometry meets the
baseline's purpose.

## Recommended dimensions and states

Mark untested conditions explicitly. Additional tested sizes may be reported
as individual observations. Verify History 5/10/20/50 rows at heights suitable
for every row, Graph 1/3/6/12 months, and EMPTY title/no-title, opacity 0/0.5,
wide/tall aspects. Show valid SCORE 0 beside missing/invalid SCORE or NOTES at
the same size and describe how the score-rate bar or dial remains unknown
rather than zero. For each long title, artist or PLAY OPTIONS string used to
certify a size, record its complete visible readback. A full DOM value with a
clipped or ellipsis-marked painted tail is not a verified full-information
display. For Score at its recommended and package-preview sizes, transcribe
every visible label, count, rank distance, bar tick, clear state, timing count
and play option solely from the unscaled native and browser images, then
compare those readbacks to the scene input. Note touching or ambiguous rows
even when the text is present in the DOM; repair and rerender before calling
the size verified.

## Objective checks and visual review

Record ZIP/manifest/resource/state/field/preview checks and their commands.
For final evidence, state the source revision or content hashes used to build
the ZIP and the time/hash of the preview and native/browser renders. Rerun
them after the last Wasm, CSS, font or asset edit; older captures are trial
evidence, not proof of the packaged source. Keep final scene inputs and
selected native/browser evidence under `evidence/`. Temporary trial captures
can explain rejected choices but cannot be the sole final proof.
Link native and browser images or video with content, dimensions and
timestamps. For each widget, link information-filled renders at its
recommended size; include normal-scale views of fixed labels, primary values,
the frame, the declared default size and motion time samples. Add a
package-preview-size check for Selection, Score and History Graph. Link a
background-off image of the information-bearing widgets so their world is
judged without canvas art. For every claimed continuous effect, identify its
pixel region and low/high phase timestamps; record native and browser pixel
differences there and confirm the change is visible at actual size. Record
that data regions remain still. For the exceptional-state gate, include the
native and browser History images from review cases 02 (AAA) and 08 (FULL
COMBO); the composite review video shows only one History case.
Link actual-size native and browser Score/History comparisons from the
controlled `rank-a`/`rank-aa`/`rank-aaa` scenes. Name the visible positive cue
added at AA and the further material cue added at AAA; different letters alone
are insufficient. Record the clear, judgment and timing meanings separately.
Clear types form a progression. PGREAT has a stronger positive cue than
GREAT/GOOD; other judgments have positive and negative groups. FAST/SLOW are
equal-weight directions. A special AAA/FC badge does not stand in for all
semantic states. Link the final native/browser `background-off`/
`background-static` scenes and, for a moving canvas, `background-animated`.
A still canvas records why an animated canvas control is omitted. Read the
material cue on the Select, Score and History faces with background off;
compare background-only regions at low and high animation phases while the
widget data stays still. For each applicable
[finish gate](basic-design-system.md#self-assessed-finish-gates), record a
pass/fail observation from these images, naming the visible cue or defect
rather than writing only "visually inspected". For each failure, record the
production change and the rerendered result. Record the unscaled small-type
readback and any mismatch before consulting the scene. Note exceptions and
remaining limits. A passing DOM rectangle is not visual evidence. An existing
skin comparison is needed only when a separate evaluation task requests it.
