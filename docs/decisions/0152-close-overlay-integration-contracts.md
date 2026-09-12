# ADR 0152: Close overlay integration contracts at renderer boundaries

## Status

Accepted

## Context

ADR 0150 established browser and fake-Wayland integration as the routine overlay gates, but the
first implementation still proved several components independently rather than their contracts as
a complete route. A newly added browser canvas could navigate its iframe before the backend replica
contained that draft, a delayed stage-control response could arrive during a later drag, and the
fake Wayland workload did not enter the same normalized event and frame functions as the production
application. Performance counters also conflated Wasm execution with JSON decoding and treated a
layout pass as resource lookup.

## Decision

An editor iframe marks its initial request as an editor bootstrap. That request waits, for at most
the existing two-second control timeout, for the accepted backend replica to contain the canvas.
It does not create a second draft or editor authority. Ordinary display canvas requests remain
immediate and return not-found for an absent canvas.

Every control request retains the exact draft it submitted. An Update or Save response may replace
the Dioxus `EditorSession` draft only while that submitted draft is still current. A response for an
older request remains stale by request ordering; a response that arrives after a newer local pointer
move is stale by draft identity even if the newer persistence effect has not yet been sent. The
browser integration gate delays one Update response into the following drag and requires the
editor handle, canvas WebSocket presentation and iframe Wasm DOM to converge on the final geometry.
Wayland protocol events enter one `dispatch_native_event` normalization function in production and
fake tests. Production and fake frame paths also share Dioxus polling, retained skin reconciliation,
resource-message handling, Blitz resolution, scene construction and presenter admission through
production-owned stage and display turn functions. Display runtime/tree construction is shared too.
Configure events use the same dispatcher and forward logical
output size to the Dioxus authority in both environments. The fake
provides only configure/input/frame/surface facts and records explicit create, present, commit,
unmap, wake and close operations; it does not decide editor selection, revisions or lifecycle.

The 60 and 120 Hz workload cases each advance one second of frames with multiple canvases and
multiple widgets, including an animated background. They require stable projection,
canvas conversion, package open/clone, resource lookup, runtime creation, skin input and tree
reconciliation on unchanged frames, while Dioxus poll, layout, scene, fake present and fake commit
continue for every live output. The unchanged counters are checked across all retained stages, so
the animated canvas cannot rerender or recreate an unrelated canvas presentation/runtime. Wasm
execution and JSON decode/validation are measured inside the runtime;
package and resource lookup measurements live at their actual cache/provider boundaries.
The native summary starts a bounded per-frame delta record before projection acceptance and Dioxus
polling and closes it after presentation. Every required phase is represented by measured work,
zero work or an explicit unmeasured marker, together with live canvas and widget counts. The production
renderer reports scene construction separately from its combined GPU/present call; its dependency
does not expose `wl_surface.commit` timing, so that phase is explicitly marked unmeasured rather
than recorded as zero. The fake surface adapter has a real, separately measured commit operation.

A checked-in headless nested Scroll task is an explicit, non-routine observation. It creates two
outputs at different refresh rates and runs the production Wayland editor with multiple retained
canvases and widgets. Through production `EditorInput` transport it moves a canvas to the other
output, changes visibility and deletes another canvas. A dedicated test client uses
`zwlr_virtual_pointer_v1` to close and reopen the editor and to inject
motion, primary and secondary buttons, and an axis event through the compositor into production
input normalization. Each deterministic action first selects its named canvas through production
transport and then enters the sole `EditorSession` authority. The task requires each accepted action
revision to reach a later paint on every receiving stage, and independently correlates a
non-fixture changed authority revision through a receiving output stage to its later paint. It requires sub-250 ms
input-to-paint latency, at least 55 effective paints per second on both
outputs, all required per-frame work phases, and the expected unmap/complete lifecycle evidence.
Browser plus fake Wayland remain the
normal completion gate; the nested task checks that the fake scenario still corresponds to the
production protocol, surface and frame path.

## Consequences

New canvas startup, delayed control replies and reverse stage delivery have explicit integration
oracles instead of timing assumptions. Native performance reports attribute retained-resource work
at the boundary where it occurs without reducing Dioxus polling or repaint cadence. The only
browser/native differences introduced here are HTTP bootstrap, Wayland protocol normalization,
separate stage transport and renderer/surface presentation. One renderer-adapter normalization
preserves the focused standard DOM control's selection and active IME preedit across a Blitz rebuild:
the browser retains those DOM facts, while the current Blitz reconciliation resets them. It does not
interpret editor fields or create writable state outside the Dioxus session.
