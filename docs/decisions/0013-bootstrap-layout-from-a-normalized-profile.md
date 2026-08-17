# ADR 0013: Bootstrap the shared layout from a normalized profile

- Status: accepted
- Date: 2026-08-18
- Supersedes: ADR 0009's requirement to observe multiple peer profiles before defining the initial shared layout

## Context

The fixed canonical contract already specifies contiguous RGB8 1920x1080
logical game pixels. A lossless OBS/vkcapture recording supplies FFV1
`yuv420p`, limited-range BT.709 frames at exactly that geometry. Waiting for
Portal and Gamescope recordings would prevent an offline recognition spike,
even though later routes can be calibrated to the already independent
canonical contract.

Treating decoded observed RGB as canonical without an explicit transform would
hide the capture-domain boundary. Letting recognizers consume observed frames
would also make every field implementation profile-dependent.

## Decision

The current OBS/vkcapture profile receives the first versioned domain
normalizer. Its exact admitted contract is converted with the pinned FFmpeg
binary and an explicit limited-range BT.709 to full-range RGB24 transform. The
artifact binds the observed contract, capture-profile digest, canonical-frame
contract, implementation ID, filter, and FFmpeg digest.

The initial normalizer registry entry is an exact tuple, not a metadata pattern:
capture-profile digest, FFmpeg binary digest, Matroska/FFV1/yuv420p, 1920x1080,
time base 1/1000, and explicit limited-range BT.709 space, transfer, and
primaries must all match. A new or changed capture profile requires another
calibrated registry entry even when its dimensions and codec look identical.

Only the normalizer output may construct `CanonicalFrame`. The recognition API
does not accept `ObservedFrame` or unbound decoded RGB. The initial shared
result layout is independently measured from stable normalized frames in the
scorepeek recording. It is owned by the canonical contract rather than the OBS
profile.

Future Portal, Gamescope, or OBS profiles must each provide a deterministic
normalizer into this same canonical geometry. Adding a profile calibrates its
normalizer; it does not create a route-local layout or move existing ROIs.
Changing actual game geometry or field semantics requires a new shared layout
version and replay.

## Consequences

- Offline recognition can proceed from the existing recording before another
  capture route is available.
- The first profile is useful calibration evidence but is not a pixel
  correctness reference, default route, or supported-profile proof.
- Multi-profile, lifecycle, performance, and profile-disjoint gates remain
  required before capture support claims.
- Recognition and layout code remain profile-independent by construction.
