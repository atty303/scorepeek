# ADR 0129: Overlay the editor panel and manipulate canvases directly

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0128's fixed 320-pixel sidebar, scaled preview, sidebar-row canvas movement, and undifferentiated settings list

## Decision

The Wayland and OBS workspace editor uses the output itself as a one-to-one canvas preview. A dark,
nearly opaque panel overlays its left edge instead of reserving space or scaling the preview. Its
width is one fifth of the logical output, clamped from 360 through 480 pixels. A small icon button at
the upper-left remains available while editing and toggles the whole panel. The title, semantic-screen
preview, canvas selector, WIDGETS/CANVAS tabs, and save actions stay at stable edges; independently
scrollable lists and expandable low-frequency sections contain the remaining controls.

Every selectable button exposes its state through both visible treatment and ARIA state. Canvas
selection and canvas enablement are distinct: the selected row identifies the draft target, its lamp
shows enablement, and `CANVAS ENABLED` changes that value under `MANAGE CANVAS`. Visibility is expressed
as `ALL SCREENS` or `SPECIFIC SCREENS`; choosing the latter initially selects the current preview screen,
and removing its final screen returns to all screens. Wayland output selection belongs to the CANVAS
tab and lists connector name, model, and logical size. The panel, tab, and expanded-section state
survive a Wayland surface handoff.

Right-click in normal display enters editing. While editing, right-drag on a visible canvas selects and
moves it directly on the four-pixel grid. Left-drag moves or resizes widgets, and the canvas corner
handles resize the selected canvas. Right-drag and left-drag do nothing to persisted geometry outside
editing. Geometry retains one workspace-wide undo slot, including a directly moved canvas that was not
selected when the gesture began. No keyboard input or z-order control is added.

Draft state is visible as a dot on the panel toggle and title. SAVE AND CLOSE and DISCARD are enabled
only for a dirty draft; SAVE receives the primary treatment only then. PREVIEW ACTUAL keeps only its
return control. Inactive sessions continue to render labeled sample data inside the editor. OBS uses
the same arrangement within Browser Source Interaction at `/overlay`; it omits only the Wayland output
selector.

## Consequences

The output remains a faithful placement surface even on small displays, while the panel may cover part
of it until collapsed. Common canvas, widget, draft, visibility, and state semantics remain shared
between backends. Host-specific behavior is limited to Wayland output enumeration and native pointer
and surface integration.

## Verification

Development verification covers the shared backend dirty flag, responsive panel bounds, embedded OBS
stage controls, JavaScript syntax, native DOM layout, and the existing atomic workspace tests. Target
visual checks cover each connected Wayland output without leaving the overlay running after capture.
OBS Browser Source Interaction remains a separate target gate from a representative 1920x1080 browser
render.
