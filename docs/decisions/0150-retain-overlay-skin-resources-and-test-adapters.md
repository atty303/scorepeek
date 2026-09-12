# ADR 0150: Retain overlay skin resources and test renderer adapters

## Status

Accepted

## Context

The Dioxus editor authority and revisioned stage replicas from ADR 0149 removed duplicated editor
state, but the Wayland rendering path still rebuilt configuration projections and opened or cloned
expanded skin packages along frame-paced paths. Editor stages also created an unused bootstrap skin
runtime in addition to the canvas-owned preview runtimes. These costs are unrelated to polling an
unchanged Dioxus VDOM and can delay input by orders of magnitude more than a 60 or 120 Hz repaint.

The browser editor could update selection geometry before its canvas iframe received the matching
presentation through the backend WebSocket. This made a drag move only the editor handle until a
later interaction. Unit tests for the reducer, skin runtime, renderer and protocol adapters did not
exercise their lifecycle contracts together.

## Decision

The browser behavior is the editor renderer contract. OBS is its one-output CEF deployment adapter;
ordinary browser integration tests are the acceptance gate unless an OBS-only API is introduced.
Wayland-specific normalization is permitted only at an intrinsic protocol, surface, separate-VDOM
transport or renderer-conformance boundary. A Blitz workaround requires a test that first
demonstrates its difference from browser behavior and must not add editor semantics.

The browser authority reactively sends each canvas iframe a complete effective same-origin skin
specification tagged with `session_id` and `revision`. The iframe accepts a newer revision atomically, accepts a new session,
and rejects stale revisions from the current session. A delayed WebSocket presentation cannot
replace newer authority geometry. The iframe remains a transport replica, not writable editor
state.

An installed skin ZIP and its expanded package are heavyweight immutable resources. The Wayland
process opens and validates a package once per skin identity into a non-reactive cache. Empty-editor
bootstrap and projection fallback consume its retained skin identity instead of reopening ZIPs. Signals,
memos and projection transport contain only skin identity and presentation data, never package,
resource or runtime bytes. Each visible canvas owns exactly one Wasm runtime and retained skin tree
from mount through unmount. Changing a canvas skin replaces only that canvas runtime. Deleting or
hiding a canvas, closing the editor, moving ownership between output stages, or removing an output
unmounts the retained tree and drops its runtime before the surface is unmapped and its worker is
joined. A process-wide canvas-owner lease prevents reverse delivery during output reassignment from
mounting one canvas in two output instances; the new owner retries after the old owner unmounts.

Every permitted Wayland frame continues polling Dioxus and may repaint animation. An unchanged
session revision does not rebuild or clone its stage projection, convert canvas configuration,
reopen a package, recreate a runtime or surface, or reconcile an unchanged skin input. Passive
pointer motion outside drag or placement is rejected before acquiring a writable editor signal,
remains a defensive reducer no-op, and does not advance revision or publish a new projection. Drag
and placement moves remain revisioned.

Routine Wayland integration tests use a semantic-free fake adapter around production input
normalization, Dioxus authority and projection, canvas/runtime lifecycle and surface admission. The
fake supplies only compositor facts and events and asserts the complete desired-to-unmap lifecycle,
including multiple outputs and canvases, reassignment, visibility, deletion, close and reopen,
output removal, stale revision rejection, nested wheel targeting, and stable 60/120-frame resource
counters. Nested Scroll may be used to observe and construct scenarios, but is not part of the
routine test suite. A renderer-conformance test records that unadapted Blitz wheel dispatch follows
stale hover instead of the wheel coordinates before verifying the narrow normalization. General
browser integration exercises the production HTTP, WebSocket, iframe, worker and Wasm route,
including drag geometry, effective property replacement and nested scroll.

For these editor contracts, this decision supersedes ADR 0148 and ADR 0149 where their consequences
required actual OBS or live Wayland composition as a separate completion boundary. Those systems
remain useful for investigating adapter-specific failures, but absence of that redundant run is not
an unverified product responsibility.

## Consequences

Dioxus polling and skin animation remain frame-paced instead of being throttled to hide work.
Steady-state cost is attributable to actual VDOM, skin render, Blitz/Vello and presentation work,
while package, runtime, projection and surface lifecycle changes are observable counters tied to
real state transitions. Browser and native share the reducer and semantics; native code contains
only tested adapter normalization where Blitz currently differs from the browser contract.
