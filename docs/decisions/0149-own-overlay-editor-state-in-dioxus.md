# ADR 0149: Own overlay editor state in Dioxus

## Status

Accepted

## Context

ADR 0144 shared the editor reducer and DOM between Wayland and OBS, but the native adapter still
copied editor fields into backend-owned mutable state and then synchronized individual Dioxus
signals. Output changes, selection, gestures and panel state could therefore be observed in
different orders by separate Wayland stages. Polling the native Dioxus DOM only after selected
field transitions also diverged from the browser frame model and let unrelated reactive changes or
preview animation remain stale.

## Decision

One Dioxus editor instance exists for each backend editor lease. Its
`Signal<EditorSession>` is the only writable authority for the complete editor: draft, active
output, canvas and widget selection, Inspector view inputs, panel state, title and IME state,
gesture state, undo, validation and notices. Every input is a typed `EditorInput` reduced into that
signal. Backend code does not interpret editor actions or fields.

Active output, selection, Inspector view, selected canvas and per-output `StageProjection` values
are read-only `use_memo` derivations. A memo is neither an independently writable state nor a
synchronization target. Components in the authority VDOM subscribe only to the derivation they
need.

OBS uses the same session and reducer with one logical output. Wayland uses the same authority but
has one rendering Dioxus instance per connected output. The authority publishes a complete
`StageProjection` carrying `session_id` and monotonically increasing `revision`; each stage replaces
its transport replica atomically and rejects an older revision from the same session. Replicas are
never editor authorities.

Pointer, wheel, keyboard and IME input enters Dioxus and is reduced into `EditorSession`. Drag start,
every move, release and cancel therefore advance the session projection as applicable. Persistence
effects are emitted only at reducer-defined transaction points. Dioxus rebuild work and skin work
coalesce at a Wayland frame, and every Wayland frame performs a Dioxus poll even when no manually
selected field changed. An idle poll does not damage or repaint the surface. Frame-only callbacks
continue preview animation.

The backend retains only message transport, output and surface lifecycle, normalized input
delivery, painting, and acquire/keepalive/update/commit/release persistence effects. Ordered input
messages are not dropped. Wayland stage transport is latest-complete-projection delivery, and the
causal diagnostic path records received input, reducer revision, projection publication, stage
receipt, Dioxus rebuild and paint.

Backend-specific code is limited to unavoidable protocol, surface, separate-instance transport and
renderer adaptation. A renderer adapter may normalize DOM pointer, wheel, keyboard, IME, focus and
paint behavior to the browser contract, but it must not implement native-only selection, picker,
field, drag, output-switching or other editor semantics as a fallback.

The title and skin-property controls emit the same shared Dioxus actions on every renderer. Draft
text, composition and invalid intermediate property values live in `EditorSession`; the Wayland
adapter only converts protocol input to DOM events and reads the currently focused DOM text
control's value, selection, composition range and rectangle when updating the Wayland text-input
protocol. It applies each text-input-v3 batch in browser order—clear the prior preedit, delete
surrounding committed text, commit, then install the next preedit—and reports only the generic
focused-control composition lifecycle to the shared reducer. Unchanged surrounding text and cursor
state is not recommitted to the protocol unless a pending `done` acknowledgement requires it.
Field input and its commit message remain ordered transport inputs; commit consumes and validates
the current authority-owned draft against current output, geometry and skin-property constraints in
one reducer transaction rather than trusting a possibly stale stage prop. A rejected commit retains
the draft, and an input update preserves composition until the shared composition-end action clears it.
Display-mode secondary clicks are ordinary Dioxus context-menu events, so the shared
`EditorSurface`—not the Wayland event adapter—requests editor entry.
Legacy output discovery likewise sends the
controller's complete response to the shared reducer. Native code does not clone or rewrite a
canvas to manufacture an editor-state fallback. Reducer-owned notices are rendered from the same
stage projection on native and OBS.

The former native split-signal state, manual field synchronization, output-selection epoch, and
model extraction/application functions are removed rather than retained as compatibility paths.

## Consequences

Native and OBS behavior now follows the same reducer authority and browser-like reactive model.
Output switching cannot leave a second backend-owned editor state behind, selecting the current
output is a no-op that preserves selection, and returning to an output can select its canvases
normally. Selecting a canvas or widget on a connected peer output activates that output in the same
reducer transaction and makes a screen-specific selection visible, while the explicit editor-entry
preview context is preserved until an explicit navigation action. A Wayland stage may
render a full revisioned snapshot slightly after the authority has
advanced, but it cannot partially combine fields from different revisions. Actual compositor,
multi-output input and OBS Interaction behavior remain target-live verification boundaries.
