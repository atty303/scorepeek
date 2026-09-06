# ADR 0135: Make the game screen the overlay editor context

- Status: Accepted
- Date: 2026-09-06
- Supersedes: ADR 0125's canvas enablement, ADR 0128's schema-v3 compatibility and PREVIEW ACTUAL behavior, and ADR 0129's peer WIDGETS/CANVAS tabs and separate visibility controls

## Decision

Overlay configuration schema version 4 removes `canvas.enabled`. A canvas is visible on every known
game screen when `show_on` is omitted, visible only on the listed screens when it is non-empty, and
hidden everywhere when it is an empty list. Version 3 migrates automatically: an enabled canvas keeps
its existing `show_on`, while a disabled canvas becomes `show_on = []`; the `enabled` key is removed.
Version 2 first performs the existing version-3 migration and then the version-4 migration. A backend
must retain at least one valid canvas, but every canvas may be hidden.

`GAME SCREEN` is the editor's top-level context and uses the full labels MUSIC SELECT, MODE SELECT,
DECIDE, PLAY and RESULT. The canvas list is always present in stable configuration order and shows raw
canvas IDs. Each row has one toggle for whether that canvas appears on the current game screen. There
is no aggregate visibility editor and no separate enablement state. Selecting a canvas hidden on the
current screen changes the game screen to the first screen on which it is visible, in the order above.
A canvas hidden everywhere remains selected and shows guidance instead of widget controls. Changing
the game screen selects the first visible canvas; if none is visible, no canvas is selected. A newly
created canvas appears only on the current game screen.

Canvas geometry is shared by all game screens. The panel therefore presents a vertical hierarchy of
GAME SCREEN, CANVAS SETTINGS and WIDGETS; WIDGETS is available only when the selected canvas is visible
in the current context. There are no WIDGETS/CANVAS peer tabs, visibility-impact indicator, gesture
legend, or PREVIEW ACTUAL mode. Collapsing the panel leaves the direct-manipulation outline and handles
on the output. Canvas right-drag and widget left-drag retain their existing meanings. Clean drafts show
only CLOSE EDITOR; dirty drafts show DISCARD CHANGES and SAVE ALL CHANGES AND CLOSE. The backend-wide
draft remains one atomic transaction.

The Wayland editor appears on every connected output while a workspace is open. All panels are peers:
screen, selection, disclosure, widget placement, draft and dirty state are synchronized, and controls
on any output may update the same backend draft. Each output uses one existing canvas surface as its
editor host; an output without a canvas gets a run-local hidden editor surface. Right-click selects the
canvas under the pointer; right-clicking empty output space selects the first canvas visible on the
current game screen. ADD CANVAS binds the new canvas to the output whose panel was used. Canvas rows do
not repeat output names because placement is visible on the outputs; MOVE TO OUTPUT remains an expanded
low-frequency control. The editor opens at the live known game screen, or MUSIC SELECT when no known
screen exists.

## Consequences

Users edit the screen they are looking at instead of reconciling independent preview, enablement and
visibility modes. An empty `show_on` has a useful explicit meaning, and fully hidden canvases can be
prepared without deleting them. Schema version 4 is intentionally breaking for external schema-v3
writers, but the bundled loader preserves existing runtime behavior during automatic migration.

Rendering editor chrome on every output adds run-local Wayland surfaces while editing. Those surfaces
are never written to configuration and disappear when the workspace closes. Output panels may cover
content until collapsed, but placing handles only on the real output keeps geometry spatially direct.

## Verification

Development verification covers version-2 and version-3 migration, empty `show_on`, current-screen
toggle semantics, backend-wide draft validation, JavaScript syntax, native DOM scenarios and the OBS
browser route. Actual multi-output Wayland composition and pointer delivery, and rendering inside OBS,
remain explicit target validation boundaries.
