# ADR 0009: Own game layout in the canonical frame contract

- Status: Accepted
- Date: 2026-08-16
- Supersedes: ADR 0008

## Context

The game presents one logical UI layout independently of whether scorepeek
observes it through Portal, Gamescope direct PipeWire, or another eligible
capture route. A capture profile describes observed pixels. It does not own the
game layout.

ADR 0008 correctly rejected any capturable route as a pixel correctness oracle,
but it left construction of the conceptual canonical domain underdetermined.
The first private-corpus schema then bound a capture profile, normalizer digest,
and layout profile at source ingest time and treated decoded 1920x1080 observed
frames as canonical layout evidence. That ordering required artifacts which did
not yet exist and allowed one layout per route even though geometry
normalization is responsible for mapping every route to the same game canvas.

## Decision

The versioned canonical frame contract owns the game-coordinate system and one
canonical layout artifact. Its first contract is owned, top-left,
C-contiguous RGB8 at exactly 1920x1080. A `CanonicalFrame` references that
contract and layout by immutable ID or digest; it does not duplicate all ROI
definitions in each frame.

A capture profile owns only its exact observed input contract. Immutable source
ingest and media probe/extraction therefore bind the opaque capture-profile ID
but no normalizer or layout. Extracted RGB8 frames remain observed-frame
evidence at the source dimensions until a normalizer has produced a canonical
frame.

Each versioned normalizer artifact maps exactly one admitted capture profile to
one canonical frame contract. It owns deterministic format decoding, color
conversion, viewport geometry, and bounded resampling needed for that mapping.
No capture route is the target. The target is the specified logical game
canvas. Adding a capture profile calibrates a new normalizer against the
existing canonical layout; it does not move that layout.

The canonical layout contains scorepeek-owned field ROIs, presence predicates,
and alignment tolerances. It changes only when the game UI geometry, canonical
frame contract, or field contract changes. A route, transport format, color
range, or capture setting change instead creates or changes a capture profile
or normalizer artifact.

Replay metadata binds the observed source and capture profile separately from a
`canonical_frame` binding containing the normalizer artifact, canonical frame
contract, and canonical layout digest. Recognition replay applies that binding
before ROI inspection. Profile-disjoint evaluation groups by the observed
capture profile, not by normalizer or layout.

Normalizers are calibrated only from real supported-route observations. The
construction sequence is:

1. observe at least the Gamescope direct and Portal candidates on Bazzite;
2. establish each opaque observed contract and collect independent calibration
   evidence from both peer profiles;
3. use that joint evidence to define the canonical game-coordinate/layout
   contract and calibrate one deterministic normalizer per observed profile;
4. verify alignment and semantic replay on held-out sessions for every profile;
5. admit a profile only after its independent semantic, lifecycle, and
   performance gates pass.

No learned residual adapter is part of the initial contract. A later decision
may introduce a bounded deterministic residual only after evidence shows that
the explicit geometry, color, and resampling transforms cannot satisfy shared
recognition gates.

## Consequences

- Source ingest can precede normalizer calibration without placeholder digests.
- Observed frame extraction is not canonicalization and cannot directly produce
  canonical layout evidence.
- Gamescope, Portal, and eligible OBS profiles may share one canonical layout
  while retaining different normalizer artifacts and recognition thresholds.
- A new capture route cannot redefine existing canonical ROI coordinates.
- Layout measurement resumes only after a versioned normalizer produces frames
  bound to the canonical contract.
- The pre-adoption v1 ingest, source, replay, index-plan, probe, and extraction
  schemas are removed rather than migrated or accepted through compatibility
  paths.
