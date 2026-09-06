# ADR 0137: Group canvas controls and undo drafts

- Status: Accepted
- Date: 2026-09-06

## Decision

The Wayland editor keeps GAME SCREEN and a bounded, independently scrolling CANVASES section above
the settings scroller. CANVASES owns the list, current-screen ON/OFF controls, ADD CANVAS and DELETE
SELECTED. The last canvas cannot be deleted. Canvas and widget deletion change only the unsaved
backend draft and therefore execute immediately without a second confirmation step. Wayland pointer
axis events scroll the bounded list or settings section under the pointer.

The settings scroller has independent APPEARANCE, OUTPUT and WIDGETS sections. APPEARANCE presents
SKIN and OPACITY as separate fields. OUTPUT always shows every connected output candidate. The
selected canvas row is the sole persistent canvas identity heading.

The footer always reserves its left side for one UNDO control. The right side contains CLOSE EDITOR
for a clean draft or DISCARD CHANGES and SAVE ALL CHANGES AND CLOSE for a dirty draft. UNDO stores
one complete backend-draft snapshot before the most recent effective command or pointer gesture.
It covers every persisted canvas and widget field, consumes the snapshot when used, and has no redo.
A no-op does not replace an earlier undo snapshot. Navigation such as canvas selection, GAME SCREEN
preview selection, scrolling and disclosure state does not create or replace an undo snapshot. When
undo makes the draft equal to the saved document, the existing backend comparison makes it clean.
Closing a clean editor discards any remaining undo snapshot before the next lease.

Left-clicking the visible area of an unselected preview canvas selects that canvas and consumes the
click; widget interaction begins with the next click or drag. Overlapping canvases have no selection
ordering guarantee and introduce no z-order policy.

This decision supersedes ADR 0129 only for geometry-only undo and the selected-canvas settings
layout, and ADR 0135 only for the unsectioned canvas list.

## Consequences

The editor no longer needs geometry-specific restoration, canvas-management disclosure state,
output disclosure state, or pending deletion confirmation state. Restoring a whole draft also makes
canvas and widget additions, deletions, appearance, visibility, output and geometry obey one rule.
The backend lease, atomic save, discard, current-screen membership and direct-manipulation contracts
remain unchanged.

## Verification

Focused native tests cover full-draft restoration, no-op preservation and direct preview selection.
The checked-in native visual scenario records the CANVASES, APPEARANCE, OUTPUT and footer controls in
its selector-layout artifacts. Live Wayland input remains a separate target-host boundary.
