# ADR 0145: Restructure the overlay editor around object inspection

## Status

Accepted

## Context

ADR 0135 made GAME SCREEN the editor context, but the resulting panel exposed output, canvas,
widget, appearance and rendering controls as one long column. The full-height collapse gutter and
title block consumed scarce preview space, canvas IDs were the only labels, geometry was editable
only on the surface, and visibility was limited to the current screen. ADR 0144 unified the native
and OBS editor session, so both backends can now share a single object-oriented editor UI.

## Decision

The shared Dioxus editor uses four regions: a compact ContextBar, an independently scrolling
`Workspace → Output → Canvas → Widget` ObjectNavigator, an independently scrolling Inspector, and
an ActionBar confined to the panel width. The panel remains one fifth of the logical output clamped
to 360–480 px. Closing it removes the panel and leaves only a small reopen button at the top left.
GAME SCREEN is a custom inline picker in ContextBar. Renderer cadence and other runtime settings are
outside this editor.

Canvas receives a persisted, non-empty display `name`, unique within a backend workspace. Schema v8
migrates existing canvases in stable configuration order to `Canvas 1`, `Canvas 2`, and so on,
independently per backend. Internal canvas IDs remain stable. New canvases use the first unused
`Canvas N`. Widget labels are derived from kind: the kind alone when unique, otherwise kind plus its
one-based configuration-order occurrence. Widget labels are not persisted.

Inspector owns canvas name, geometry, visibility, appearance, output assignment and deletion, and
widget geometry, kind-specific settings, style and deletion. Canvas visibility is an explicit
six-screen toggle group with All and None actions; this supersedes ADR 0135's current-screen-only
visibility control. All inspector sections begin expanded and retain expansion only for the editor
session. Name and geometry fields show local validation errors and prevent Save & Close while
invalid. Geometry is bounded by the assigned output or parent canvas and uses the existing 4 px
grid. A canvas dimension may end exactly at a non-grid assigned-output edge so that a full-output
canvas remains exact. Canvas minimum size also contains every child widget.

Add Canvas immediately creates a full-output canvas for the current screen. Add Widget opens an
inline kind picker; choosing a kind immediately creates its default size centered in the selected
canvas and selects it. Overlap is allowed. Delete Canvas and Delete Widget are immediate and remain
covered by the existing one-level document Undo; there is no confirmation or Redo.

Controls are shared Dioxus components rendered by both native and OBS routes. Browser-native select,
dialog and popover controls are not used. Cyan denotes selection and focus, amber denotes an unsaved
draft, and red is reserved for destructive actions. Keyboard behavior follows ordinary focus order,
Enter/Space activation, picker navigation, and Escape dismissal without global shortcuts.

## Consequences

Schema v8 is a persisted format change, but migration is deterministic and requires no operator
choice. Users can distinguish canvases without changing stable IDs and can edit exact geometry and
all screen visibility from one inspector. The editor uses substantially less fixed chrome, while
the preview retains the existing 360–480 px panel cost when open. Live Wayland composition/input and
OBS Browser Source rendering remain separate target verification boundaries.
