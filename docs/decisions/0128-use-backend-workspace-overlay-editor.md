# ADR 0128: Use a backend workspace for overlay editing

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0125's per-canvas editor lease and ADR 0127's in-canvas editor, z-order, output fallback persistence, and schema-v2 compatibility rules

## Decision

Overlay configuration schema version 3 removes canvas and widget `z`. Peer ordering is unspecified;
layouts are expected to avoid overlap. Version 2 is migrated automatically by removing only `z`
and retaining all other values. A backend editor holds one lease and one draft for every canvas of
that backend. Draft changes update the running preview but do not write TOML. SAVE validates and
atomically replaces the backend canvas set once; a failed save retains both draft and lease. DISCARD
releases the draft without writing it. Geometry has one global undo slot and uses only a 4-pixel grid.

The editor is a fixed dark 320-pixel sidebar beside an output-coordinate preview. The preview scales
only when the remaining area cannot contain the output, reports that scale, and keeps font and widget
layout sizes unchanged. It shows all canvases for the selected output, emphasizes the selected canvas,
and allows SELECT, MODE, DECIDE, PLAY, and RESULT to be previewed independently of live recognition.
When no active session data exists it uses fixed sample data and labels that fact only in editor chrome.
PREVIEW ACTUAL temporarily hides the workspace except for a return control.

Canvas movement starts from its sidebar row. Widgets move from their body. Canvases and widgets resize
from four fixed-size corner handles, stay within the output or containing canvas, and may overlap.
Widget placement starts in the sidebar, follows the pointer, and commits at the next preview click.
History list count and graph month range are the only widget settings. Canvas and widget deletion use
an inline second click. Canvas creation, deletion, and enablement live under the low-frequency settings.

Wayland right-click opens this workspace; `--overlay-wayland-edit` enables Wayland and opens it at
startup as a recovery path. The editor occupies the selected output surface and can move a canvas to
another enumerated output. Missing configured outputs choose deterministically: a named output that
fits, otherwise the largest output. The runtime shrinks the canvas boundary if needed, opens a fallback
draft, and does not rewrite TOML. SAVE adopts the fallback. DISCARD keeps TOML unchanged and suppresses
the affected canvas for that run.

OBS uses `/overlay` as the single full-screen Browser Source. Its normal display and Browser Source
Interaction share the same URL; right-clicking Interaction opens the same workspace over the stage.
Stable `/canvas/<id>` URLs remain display-only and direct editing attempts explain that `/overlay` is
required. OBS and Wayland share canvas semantics, draft protocol, widgets, skins, sample data, and
editing structure; backend-specific code is limited to host surface and pointer integration.

## Consequences

Editing a screen layout is one coherent transaction, so cross-canvas changes cannot leave a partially
saved backend. OBS setup needs one full-screen source, while Wayland can recover from a disconnected
output without first editing TOML. Removing z-order also removes a promise that layer-shell cannot
provide consistently. Existing schema-v2 documents are adopted automatically; schema v1 is rejected.

## Verification

Development verification covers exact v2-to-v3 migration, backend draft leases, atomic commit rollback,
display-only canvas URLs, the embedded stage bundle, output fallback selection, shared widget rendering,
and workspace DOM compilation. Target validation remains required for Wayland pointer and cursor behavior,
output moves, fallback save/discard, scale and opacity, as well as OBS Browser Source Interaction.
