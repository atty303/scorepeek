# ADR 0147: Refine the overlay editor visual hierarchy

## Status

Accepted

## Context

ADR 0146 completed the shared native and OBS editor component system, but its first visual
implementation applied nearly uniform 3--6 px gaps, 9--12 px type, bordered controls and similar
surface treatments throughout the panel. Object ownership, section boundaries, primary actions and
metadata therefore competed at almost the same visual weight. The one-line Context Bar also changed
its available picker width when the full unsaved badge appeared. On the canvas, selection outlines
could disappear into either light or dark skin artwork and did not identify the selected object's
complete geometry.

## Decision

This decision supersedes ADR 0146 only for the editor's visual tokens and presentation details. The
shared component tree, behavior, information architecture, persistence and skin rendering contracts
remain unchanged.

The editor uses a balanced desktop density: 13 px body text, 11 px secondary text, approximately
36 px controls, and a 4/8/12/16 px spacing scale. System UI is the ordinary typeface. Monospace is
limited to coordinates, dimensions, stable IDs and color codes. Uppercase with letter spacing is
limited to pane headings and short metadata labels.

Quiet graphite surfaces, small four-pixel corners, low-contrast borders and a shadow only at the
panel edge keep the editor visually subordinate to the selected skin. Controls use filled neutral
surfaces whose borders become prominent only for focus, selection and invalid state. Actions have
three visual levels: Save and Close is primary, ordinary operations are secondary, and disclosure,
aggregate visibility, Undo and Discard are tertiary. Cyan remains focus and selection, amber remains
dirty state, and red remains destructive.

The one-line Context Bar retains the complete game-screen picker and represents the active output
plus dirty state as one compact status cluster. The active output remains visible and truncates when
necessary; an amber dot, rather than a full badge, reports an unsaved draft without changing picker
width. The panel toggle, picker trigger and output cluster share one top alignment whether the picker
is open or closed.

The Object Navigator occupies `clamp(180px, 24vh, 260px)` and the Inspector receives the remaining
panel height. Navigator ownership uses compact indentation and low-contrast tree guides rather than
depth-proportional empty columns. Inspector sections remain flat and use whitespace plus minimal
dividers instead of cards. The Inspector heading does not repeat the selected object name already
shown by the Navigator selection and Identity section. Navigator and Inspector retain independent
scrolling.

Geometry presents X, Y, Width and Height as four equal fields in one row; each field is sized for
the at-most-five-digit values rather than consuming half the Inspector width. Fit to output is
placed in a separate action row with a 12 px gap below the fields, so it reads as an operation
rather than a fifth geometry field or an attached part of the Width/Height controls.

Every selected canvas or widget uses an artwork-independent dark outer edge, cyan inner edge and
clear four-corner resize handles. One noninteractive selection label always shows the selected
object's display label and complete viewport-relative `x,y · width×height` geometry. The existing
empty-widget geometry label remains for unselected empty apertures; the selection label replaces it
for the selected empty widget.

## Consequences

Native and OBS continue to render one shared DOM and stylesheet, now with a stable visual hierarchy
at the panel's 360--480 px width. The change adds no editor state, backend branch, saved setting,
skin redesign or new dependency. Development-host verification must inspect production native paint
artifacts and the browser/OBS route across normal display, multiple canvases, panel scrolling,
selection, dragging and resizing. Actual Wayland composition/input and rendering inside OBS remain
separate live boundaries.
