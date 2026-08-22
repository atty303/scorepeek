# ADR 0027: Acquire PipeWire sources behind a common receiver

- Status: accepted
- Date: 2026-08-22
- Supersedes: ADR 0009 and ADR 0013 only for treating a future OBS path as an eligible scorepeek capture profile; ADR 0013's existing offline OBS/vkcapture recording profile remains valid

## Context

The first live capture slice needs low-level video reception without making
GStreamer or OBS part of scorepeek's runtime. OBS will normally capture the
game independently through obs-vkcapture for streaming. Routing scorepeek
through OBS would add source, scene, plugin, or virtual-camera lifecycle and
would couple recognition availability to the broadcaster.

Gamescope direct and a future ScreenCast Portal session both expose video
through PipeWire, but they do not acquire the connection in the same way.
Gamescope publishes an externally owned node on the ordinary PipeWire remote.
Portal grants a session-scoped remote file descriptor and node identifier.
An operator-selected or future producer may require another explicit source
acquisition policy. Format negotiation, buffer reception, timestamps, and
backpressure do not need to be reimplemented for each policy.

## Decision

Scorepeek separates PipeWire source acquisition from PipeWire frame reception.
A selected source provider acquires one lifetime-bound source lease containing:

- the default remote or an owned remote file descriptor;
- an exact node identifier or deterministic node selector;
- the immutable opaque capture-profile identifier;
- provider-specific lifetime ownership needed to keep the source valid; and
- secret-safe provenance needed to diagnose source selection and loss.

The common receiver consumes that lease and exclusively owns PipeWire stream
creation, format and buffer negotiation, bounded latest-frame handling,
sequence and monotonic timing, and source-loss notification. It emits owned
`ObservedFrame` values. Source providers do not resize, normalize, recognize,
or silently choose another provider. The receiver does not assign a capture
profile from negotiated caps alone.

The first vertical spike implements only a Gamescope provider against the
default PipeWire remote. Portal is deferred to a later provider that owns its
Portal session together with the granted remote file descriptor and node ID.
A custom provider may later accept an explicit registered source, but an
unknown source may be used only for probe diagnostics until it has a calibrated
capture profile and normalizer.

Provider selection is explicit for a run. Acquisition failure, ambiguous node
selection, source removal, caps drift, stream error, and reconnect exhaustion
fail closed; scorepeek never falls back to Portal, OBS, or another custom
source. Reacquisition creates a new capture generation even when it resolves to
the same node metadata.

The capture path is subject to the application observability contract. One
capture diagnostic run distinguishes source acquisition, registry discovery,
stream negotiation, first-frame reception, steady reception, and shutdown.
Stable error types distinguish unavailable or ambiguous sources, remote
connection failure, unsupported format or memory type, source loss, stalled
frames, timeout, cancellation, and receiver failure. Allowlisted observations
contain bounded source type, node metadata keys, negotiated video contract,
memory type, sequence/timing aggregates, drop counts, and lifecycle events;
they do not contain pixels, arbitrary node properties, environment variables,
or command lines. Diagnostic recording failure remains non-interfering.

The Gamescope spike is evaluated while OBS and obs-vkcapture are running as the
normal concurrent streaming workload. OBS and obs-vkcapture are not scorepeek
sources, lifecycle dependencies, pixel references, or synchronization clocks.
Making either a scorepeek source requires a later ADR that supersedes this
decision.

## Consequences

- One low-level PipeWire receiver can later serve Gamescope, Portal, and
  registered custom acquisition policies without pretending their lifecycles
  are identical.
- Gamescope direct PipeWire is the only implementation and target-host spike in
  the first slice; Portal support is not required to begin or complete it.
- A shared transport does not make profiles pixel-equivalent. Every admitted
  source still needs its own opaque capture profile, normalizer, semantic gate,
  lifecycle gate, and performance gate.
- The implementation needs safe Rust bindings to libpipewire and libspa plus
  build-time `libpipewire-0.3` pkg-config metadata. Dependency adoption and the
  reproducible build environment remain a separate approval boundary.
- Success of registry discovery or synthetic reception does not establish a
  supported Gamescope profile. The live gate must cover source recreation,
  caps and memory negotiation, bounded latest-frame behavior, canonical
  production, OBS-concurrent game frametime, frame age, and resource stability.
