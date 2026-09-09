# ADR 0144: Unify overlay editor sessions and output stages

- Status: Accepted
- Date: 2026-09-09
- Supersedes: ADR 0128's backend-specific workspace state and fallback persistence, ADR 0129's canvas-hosted editor surface, ADR 0135's duplicated peer editor panels, ADR 0137's last-canvas restriction, and ADR 0140's refresh-rate editor controls
- Complements: ADR 0122's independent Wayland and OBS consumers, ADR 0125's canvas/widget composition, and ADR 0143's canvas-owned skin runtime

## Decision

Wayland and OBS keep independent canvas workspaces, but both project the same backend-neutral editor
domain. An editor session is the only authority for draft canvases, active output, preview context,
selection, gesture, one-level document undo, and panel state. Adapters provide an ordered output
catalog and normalized input; the reducer and shared Dioxus DOM/CSS do not inspect a backend kind.
Renderer transport may cache a complete generation-tagged projection, but it does not reduce actions
or become draft authority. Only one editor may own a workspace; another acquire is rejected. Wayland
and OBS workspace sessions may coexist.

The ownership hierarchy is workspace, output, canvas, widget. Game screen is an orthogonal session-wide
preview context and has six values: UNKNOWN, MUSIC SELECT, MODE SELECT, DECIDE, PLAY, and RESULT. A
runtime screen with no recognized kind maps to UNKNOWN. An omitted `show_on` means all six contexts;
an explicit list remains explicit even when it contains all six. Changing preview context preserves
canvas and widget selection. Changing the active output clears both selections without changing the
draft, preview context, or undo slot.

Every persisted canvas has a required output ID. OBS exposes one synthetic `obs-output`; Wayland uses
compositor output names. Missing outputs and out-of-bounds geometry remain visible as invalid editor
state and are never automatically reassigned or resized. Reassignment and FIT TO OUTPUT are explicit,
undoable actions. New canvases use the active output's complete logical outer bounds at `(0, 0)` and
are visible only in the current preview context. Zero canvases is valid, including deletion of the last
canvas. The empty editor retains the output and skin choices and presents CREATE FIRST CANVAS. An
enabled Wayland overlay with zero canvases opens that editor automatically through an internal
full-output bootstrap stage; no separate recovery option is required.

Outside editing, Wayland keeps one layer surface per canvas. While editing, it supplies a full-output
stage for every connected output; the active stage owns panel, handles and input while other stages are
preview-only. OBS always has one full-output stage. A skin runtime belongs to a canvas, and each stage
mounts its immutable content projection below the shared management layer. Every canvas visible in the
active preview context is shown; only the selected canvas has management handles. Canvas movement is a
secondary-button drag. Widget selection, movement and handles use the primary button. Overlap is valid.
Configuration order determines editor and OBS paint order; normal Wayland overlap order is unspecified.

Editing always uses fixed complete sample state for the selected preview context. Refresh cadence, OBS
listen settings and other runtime settings are outside the editor and its undo transaction. The process
loads one TOML document once at startup. A parent-owned writer serializes both backend commits against
that in-memory document, and a process-lifetime advisory lock rejects a second scorepeek process using
the same path. External edits during the run are outside the contract. No optimistic revision or merge
protocol is used; renderer generations are run-local and are not configuration data.

Schema version 7 removes `settings_revision`, backend revisions, canvas revisions and
`initial_placement`; permits empty backend workspaces; and requires canvas `output`. Migration is
automatic. An old OBS canvas without output receives `obs-output`. An old Wayland canvas without output
is held in memory until named outputs are discovered, then receives the first name in stable order and
the fully validated v7 document is written once atomically. With no Wayland output, migration does not
rewrite the old file. Geometry is preserved.

## Consequences

Backend differences end at output, surface, transport and input adapters. A fix to editor selection,
visibility, creation, deletion, undo or panel markup therefore applies to both routes. The empty-canvas
state is recoverable from the visual editor. Revisions and live external-edit conflict handling no
longer complicate the writer; two independent processes cannot silently overwrite each other.

The Wayland adapter must still reconcile hotplugged stages and report unavailable output state without
changing the document. Development-host browser/native evidence does not establish target compositor
or actual OBS Browser Source behavior.

## Verification

Repository tests cover schema-v7 serialization and deferred migration, UNKNOWN visibility, explicit
all-context lists, empty creation and last deletion, active-output navigation, full-output initial
geometry, same-path writer exclusion, independent workspace leases, and atomic commits. Visual checks
use the native scenario and production OBS `/overlay` in Codex Browser. Live Wayland verification uses
scorepeek inside a nested Scroll compositor and exercises empty startup, output stages, right-click,
preview selection, direct manipulation, save, discard, close and reopen.
