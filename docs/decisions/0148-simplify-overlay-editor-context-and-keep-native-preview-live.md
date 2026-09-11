# ADR 0148: Simplify overlay editor context and keep native preview live

## Status

Accepted

## Context

ADR 0147 established a balanced visual hierarchy, but the Context Bar repeated the active output
already visible at the root of the Object Navigator and prefixed the only remaining picker with a
visible `GAME SCREEN` field name. Canvas identity also placed its stable ID on a separate line, and
output choices repeated model metadata that is unavailable or unhelpful in ordinary use. Manifest
property controls added another `CANVAS STYLE` or `WIDGET STYLE` container and displayed schema
types such as `CHOICE` and `NUMBER` even though those types do not help the operator choose a value.

The native editor also bypassed Blitz's normal wheel targeting by testing only the nominal
Navigator and Inspector scroll-box rectangles. Expanded descendants can paint across the edge of
that rectangle, so a wheel gesture over a nested canvas or widget could fail even while the same
gesture over its output ancestor succeeded. Editor buttons explicitly permitted character-level
wrapping, which exposed unstable narrow line boxes after a reactive visibility or opacity update.
Finally, frame-only paints were disabled while editing, so presentation motion advanced only when
another interaction damaged the preview.

## Decision

This decision supersedes ADR 0147 where the Context Bar displayed active-output metadata. The
Context Bar contains only the panel control, the current game-screen value and the existing dirty
dot. `GAME SCREEN` remains the accessible name of the picker but is not rendered as a visible field
prefix. Output identity remains visible in the Object Navigator.

The canvas ID is shown in the Name field label rather than on a separate metadata line. Output rows
and choices show only the output name in one line. Skin properties appear directly below Appearance
or Style without an additional style card or heading, and property schema type labels are omitted.
Canvas opacity receives its own visible `Opacity` heading.

Editor action labels do not wrap. Content that cannot fit a single line is clipped or ellipsized by
the owning control instead of breaking at arbitrary characters.

Native pointer-axis events are delivered through the same Blitz wheel event and ancestor-scroll
path used by its DOM rather than selecting a scroll node from a rectangle. The checked-in visual
scenario targets a nested canvas row so this production input route remains covered.

A visible native preview continues requesting compositor callbacks while the editor is open.
Damage still waits for the next frame before an editor paint, while subsequent frame-only callbacks
advance shared presentation motion through the normal refresh cadence. This does not introduce a
second renderer or a separate editor animation implementation.

## Consequences

Native and OBS keep the same Dioxus DOM and stylesheet and retain the same draft, undo, validation,
save and output-assignment behavior. The header, Identity, Appearance and Output sections become
shallower and avoid repeated information. Native nested-row scrolling and preview motion use Blitz
and Vello's production path; the deterministic native visual debugger continues to use that same
DOM and renderer. Actual Wayland composition and input delivery remain a target-live verification
boundary.
