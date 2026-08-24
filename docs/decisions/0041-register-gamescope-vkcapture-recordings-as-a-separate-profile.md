# ADR 0041: Register Gamescope-vkCapture recordings as a separate profile

- Status: Accepted
- Date: 2026-08-24
- Complements: ADR 0036's recording simulation and ADR 0028's profile-specific normalization

## Context

`2026-08-24 14-54-57.mkv` was recorded by applying vkCapture to Gamescope and recording that
output through OBS. The earlier `2026-08-17 19-25-31.mkv` route applied vkCapture directly to the
game. Both files currently present FFV1 YUV420P BT.709 limited-range 1920x1080 media, but route,
scaling history, Gamescope provenance, and pixel domain are different. Reusing the earlier capture
profile would erase the boundary that the recording is meant to test.

## Decision

The Gamescope-vkCapture/OBS recording uses a distinct private capture context and profile. The
context binds development environment `development-machine-v1`, Gamescope
`3.16.19-128-g7282613+`, Wayland backend, 2556x1428 output, 1920x1080 nested size, 120 Hz,
auto/linear scaling, and OBS FFV1 YUV420P BT.709 limited-range 1920x1080 output. The resulting
capture-profile SHA-256 is
`f5f0c5a86b5edba6a8fd014ad85b3873be8f745c0b531d2b5b77f203770b046a`.

This profile independently binds the fixed FFmpeg BT.709-limited-to-RGB24 canonical normalizer
implementation and exact pinned FFmpeg tool digest. Because its observed recording already has
canonical 1920x1080 geometry, this normalizer performs the registered color/range conversion and
does not reuse the live Gamescope fractional geometry binding. Its normalizer artifact SHA-256 is
`75cb7c90e8fc8e430b8f3d2f33f77208971556987bc7d82066a351c3aa4d4e09`.

The recording has one reviewed failed result, so it is a profile-specific holdout and diagnostic
source. It does not satisfy the existing recognition-simulation requirement for at least three
episodes containing both failed and non-failed results, and that gate is not weakened to admit it.
The existing three-episode 2026-08-17 simulation remains the complete offline recognition gate.

## Consequences

- Identical media contracts do not collapse distinct capture routes or scaling histories.
- Unknown profile IDs, tool identities, color contracts, and source bindings continue to fail
  closed in corpus extraction and recognition loading.
- The new recording can test the shared post-canonical screen path without fabricating direct-live
  provenance or claiming a complete simulation gate.
