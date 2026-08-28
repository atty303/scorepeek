# ADR 0054: Measure Gamescope profile transforms

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0029 and ADR 0051 where they require explicit Gamescope session
  provenance, an aspect-fit transform, complete marker pixel comparison, retained launch metadata,
  or Gamescope-version equality for runtime admission

## Context

The guided setup assumed that Gamescope used one centered aspect-fit transform and then compared
almost every normalized marker pixel exactly. On the first target Bazzite machine, a 1920x1080
marker was exposed as 3840x2160 BGRx. The Wayland surface and PipeWire frame had the expected
dimensions and no decoration, but the scaling filter blended one-pixel boundaries. Fiducial
interiors and cell interiors were intact, so this was a correctable capture transform rather than
missing image content. The complete-pixel threshold rejected a usable profile.

Gamescope launch arguments, implementation-selected filters, refresh metadata, memory allocation,
and the installed Gamescope version do not define the pixels that recognition consumes. Keeping
them in the profile caused ordinary runtime compatibility checks to reject changes that the saved
normalizer did not depend on.

## Decision

`scorepeek setup gamescope --profile NAME -- GAMESCOPE_ARGS...` continues to own only its dedicated
calibration Gamescope. The marker contains nine unique, redundant fiducials across its corners,
edges, and center. Setup locates their preserved color interiors in the raw BGRx frame and fits
independent positive transforms:

```text
observed_x = scale_x * canonical_x + offset_x
observed_y = scale_y * canonical_y + offset_y
```

Every fiducial must agree with one axis-aligned transform within one observed pixel. The inferred
canonical rectangle is quantized to 1/2048 observed pixel. Padding, non-centered offsets,
fractional phase, non-integer scaling, independent X/Y scaling, and aspect distortion are accepted
when the complete inferred rectangle is inside the observed frame. A rectangle extending outside
the frame is rejected as unrecoverable crop. Mirror, rotation, shear, perspective, missing,
duplicated, or unreadable fiducials are also rejected.

Before publication, setup runs the production fractional normalizer with the inferred geometry and
checks all fiducial interiors, representative cell interiors, orientation, and RGB channel order.
Outer one-pixel equality, filter boundaries, ringing bands, aggregate exact-pixel percentage, and
mean absolute error are not acceptance criteria.

The machine-local profile is canonical schema `scorepeek-gamescope-profile-binding-v3`. It stores
only the default Gamescope PipeWire source kind, observed BGRx width and height, measured rational
source rectangle, fixed canonical RGB8 1920x1080 contract, and normalizer identity. Gamescope
arguments are used only to launch calibration and are not stored. Existing local v2 profiles must
be recreated; no migration or compatibility read is provided.

Ordinary admission observes exactly one Gamescope `Video/Source`, requires BGRx with the saved
width and height, validates the current frame's stride, memory representation, and byte length,
and requires the saved geometry to remain in bounds. It does not compare Gamescope version,
backend, scaler, filter, refresh/color metadata, calibration-time stride, or calibration-time
memory type. Recognition uses the normalized canonical frame without geometry re-estimation,
profile fallback, or threshold relaxation.

Setup proves only that the capture transform is usable. Music-select/result scene detection and
OCR from an ordinary INFINITAS run remain the authority for target support.

## Consequences

- A profile represents the correction scorepeek actually needs, not a guess about how Gamescope
  produced the frame.
- Correctable padding, scaling, and fractional placement no longer block ordinary use.
- Missing canonical pixels and transforms unsupported by the production normalizer still fail
  closed.
- Setup output and `profile list` expose only profile identity, observed dimensions, measured
  rectangle, and setup's verified fiducial count where applicable.
- Developer raw calibration samples may retain environment metadata as evidence, but that metadata
  is not profile identity or runtime admission input.
