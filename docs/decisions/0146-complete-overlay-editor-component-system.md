# ADR 0146: Complete the overlay editor component system

## Status

Accepted

## Context

ADR 0145 established the object-oriented editor regions and data behavior, but did not make the
component boundary, compact interaction contract, or fresh visual system precise enough. The
result could retain old editor chrome inside nominally new regions. It also described invalid name
input in a way that could be read as withholding the input from the draft, contrary to the accepted
live-draft editing behavior.

## Decision

This decision supersedes ADR 0145 only where the component, interaction, and name-input details
below are more specific. Its schema-v8 naming, object hierarchy, geometry, visibility, creation,
deletion, and one-level Undo decisions otherwise remain in force.

The shared Dioxus editor tree is composed from `ContextBar`, `EditorWorkspace`,
`ObjectNavigator`, `Inspector`, `Accordion`, and `ActionBar`. Reusable editor controls live in
`scorepeek-overlay-ui` as `Button`, `IconButton`, `TextField`, `NumberField`, `Toggle`,
`ToggleGroup`, `SegmentedControl`, `ListPicker`, `AccordionSection`, `NavigatorTree`,
`NavigatorItem`, and `StatusBadge`. Native and OBS adapters pass capabilities and state into that
same tree; they do not maintain backend-specific versions of the controls. Backend-specific
settings are present only when their capability exists.

The component visual system is a fresh compact desktop treatment rather than a reskin of the old
title block, full-height collapse rail, rounded cards, or decorative overlay styling. Controls are
32–36 px high, normal text is 12–13 px, and secondary text is 10–11 px. Cyan is reserved for focus
and selection, amber for dirty state, and red for destructive actions. Icon-only controls are used
only for panel minimization and tree or accordion disclosure. The open panel remains one fifth of
the logical output clamped to 360–480 px; its ActionBar occupies only the panel footer. When closed,
only a small top-left reopen control remains.

`ListPicker` expands in normal document flow, not as an absolute popover or browser-native
`select`. Selection or Escape closes it. Focus order uses Tab; buttons and toggles activate with
Enter or Space; picker choices use arrow keys, Home, End, Enter, Space, and Escape. No global
shortcut is introduced. Navigator and Inspector scroll independently. Navigator initially expands
the active branch, automatically expands the selected object's ancestors, and keeps subsequent
disclosure state only in the mounted editor session. Inspector accordions are independently open,
start open for a newly selected object, and likewise keep state only in the mounted session.

The navigator renders the actual `Workspace → Output → Canvas → Widget` ownership hierarchy,
including explicit missing-output and unassigned branches for invalid retained drafts. It owns only
selection, disclosure, and contextual creation. Inspector owns output reassignment, visibility,
properties, geometry, and immediate deletion.

Canvas name input is copied to the draft on every input event, including empty or duplicate text.
Those values remain visible as inline invalid state and disable Save & Close; they are not clamped,
silently reverted, persisted, or sent to a backend's strictly validated stage transport until the
whole document is valid again. Geometry fields instead retain an input-local string and copy only a
valid value to the draft on Enter or blur. Their bounds come from the canvas's assigned output, not
whichever output is currently active.

## Consequences

Native and OBS get the same DOM, keyboard semantics, validation behavior, and visual vocabulary.
An invalid name is now observable in the draft and remains undoable while persistence stays blocked.
Session-only disclosure state does not affect the overlay configuration. Missing output references
remain visible and repairable rather than causing canvases to disappear from navigation. Live
Wayland keyboard/IME delivery and actual OBS Browser Source composition remain target verification
boundaries.
