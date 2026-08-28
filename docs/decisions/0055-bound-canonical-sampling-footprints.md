# ADR 0055: Bound canonical sampling footprints

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0054 where it requires the continuous extrapolated rectangle itself to remain
  inside the observed frame

## Context

The measured transform maps canonical pixel-center coordinates into the observed image. A target
Gamescope session produced the valid transform `(-0.5, -0.5, 3840, 2160)` for a 1920x1080 marker
scaled to 3840x2160. Every canonical pixel center needed by the production normalizer was present,
and the half-pixel offset represented scaler phase rather than missing content.

Requiring the continuous rectangle boundary to lie in `[0, width] x [0, height]` rejected this
correctable transform before production normalization. It also conflicted with the normalizer's
half-pixel sampling convention, which clamps interpolation support at the outer observed pixels.

## Decision

Source-rectangle left and top coordinates are signed rationals. Width and height remain strictly
positive. Existing v3 profiles with non-negative numerators keep the same JSON shape and remain
readable.

Crop admission uses the production normalizer's canonical pixel-center sampling footprint. For
each axis, the transformed first and last canonical pixel centers must lie within the observed
pixel support boundaries. The continuous rectangle boundary may extend outside the frame when the
saved scale and phase still leave every required canonical pixel center reconstructible.

The check uses exact rational arithmetic after the measured transform is quantized to 1/2048
observed pixel. Setup then continues to run the production normalizer and validate fiducial and
cell interiors, orientation, and channel order. A sampling footprint outside the frame remains an
unrecoverable crop and is rejected.

## Consequences

- Valid half-pixel scaler phases such as `(-0.5, -0.5, 3840, 2160)` can be saved without changing
  the measured transform.
- Crop rejection follows the pixels the normalizer actually needs rather than a different
  continuous-boundary convention.
- Negative width or height, missing canonical pixel centers, and unsupported transforms remain
  invalid.
