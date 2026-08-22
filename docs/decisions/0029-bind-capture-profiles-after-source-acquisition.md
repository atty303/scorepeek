# ADR 0029: Bind capture profiles after source acquisition

- Status: accepted
- Date: 2026-08-22
- Supersedes: ADR 0027 only for requiring every provider lease to contain a capture-profile ID and
  for treating provider and receiver source loss or shutdown as one ownership boundary

## Context

ADR 0027 correctly separates source acquisition from shared PipeWire reception, but its lease shape
requires an opaque capture-profile ID before the first Gamescope receiver and calibration run exist.
The exact registry selector proves only which current node was acquired. Node properties and later
negotiated caps do not establish that its pixels belong to a calibrated domain normalizer.

Inventing a provisional profile ID would let uncalibrated frames cross the runtime `ObservedFrame`
boundary and would also create a false binding for the canonical diagnostic recorder. Refusing to
retain the source until calibration, however, prevents the receiver and calibration evidence from
observing the same provider lifetime.

Provider and receiver lifecycles also fail at different boundaries. Removal of the selected registry
global or loss of the provider-owned remote invalidates acquisition even before a stream exists.
Negotiation failure, stream error, frame stall, and receiver teardown begin only after the shared
receiver consumes that provider lease.

## Decision

Source acquisition first returns an explicitly uncalibrated provider lease. It owns the selected
node, remote, registry, context, loop, and provider-specific guards, but no capture-profile ID.
Selected-node removal is latched for that lease: a later node, including reuse of the same numeric
global ID, never revives or replaces it. The provider owns acquisition, registry discovery,
provider-node or remote loss, and provider shutdown observations.

The common receiver may consume an uncalibrated lease only in an explicit diagnostic or calibration
mode. That mode may retain bounded uncalibrated frame evidence, negotiated video and memory
contracts, and lifecycle measurements, but it cannot construct `ObservedFrame`, invoke a
`DomainNormalizer`, enter recognition, or claim support. Negotiated caps alone never become an
opaque profile ID.

After independently reviewed calibration evidence registers an immutable mapping among provider
provenance, observed video contract, opaque capture-profile ID, and normalizer artifact, a typed
binding may wrap a newly acquired matching lease. Only that calibrated lease can let the common
receiver emit `ObservedFrame` with the registered profile ID. Drift or an absent or ambiguous
binding fails closed.

The receiver separately owns stream creation, caps and memory negotiation, bounded latest-frame
handling, sequence and monotonic timing, first and steady frame reception, stream loss, and receiver
shutdown. Provider shutdown follows receiver shutdown. Neither layer performs blocking filesystem
I/O or encoding in PipeWire callbacks, and pixels and arbitrary properties never enter diagnostic
facts.

## Consequences

- The first provider checkpoint can honestly retain one Gamescope node without fabricating a
  capture profile.
- The next receiver slice can measure the real observed contract needed for calibration while
  remaining structurally unable to publish runtime `ObservedFrame` values.
- Provider loss and receiver stream loss have distinct typed ownership and shutdown order.
- A calibrated profile is an explicit registered artifact binding, not a hash or label derived from
  mutable node metadata or negotiated caps.
- ADR 0027 otherwise remains authoritative: Gamescope is the only first provider, Portal and custom
  providers remain deferred, and no fallback or support claim is introduced.
