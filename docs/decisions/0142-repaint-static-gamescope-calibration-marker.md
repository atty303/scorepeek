# ADR 0142: Repaint the static Gamescope calibration marker through receiver startup

- Status: Accepted
- Date: 2026-09-08
- Complements: ADR 0051's guided profile setup and ADR 0141's requested-size negotiation

## Context

Guided setup uploads the complete 1920x1080 X11 calibration marker once and then leaves the window
unchanged while the receiver attaches. ADR 0141 makes Gamescope apply its private PipeWire
`requested_size` after that attachment. Gamescope can mark the pre-transition frame corrupted and
wait for later surface damage before painting a frame in the new capture domain.

An animated vkcube client supplied that later repaint, but the static setup marker did not. The same
development-host setup entry consequently reached source acquisition and stream transitions, then
failed with `FirstFrameTimedOut`. Increasing the two-second timeout would not create another frame.

## Decision

While the setup-owned marker window is alive, issue one unchanged 1x1 X11 image update and flush it
every 100 ms. The pixel value is the marker's existing value at that location, so the calibration
image, fiducials and normalizer evidence do not change. The heartbeat exists only in the dedicated
setup marker process; ordinary Gamescope clients and the common receiver remain unchanged.

Keep the existing receiver timeout and typed failure classification. Setup still requires one valid
frame from the final requested-size contract and never accepts the corrupted transition frame.

## Consequences

- Static marker setup produces bounded surface damage until Gamescope has painted the final capture
  domain.
- The heartbeat costs at most ten 1x1 X11 updates per second for the marker's bounded 30-second
  lifetime.
- A development-host fail/pass comparison must use the same setup command and isolated profile
  root. Success requires profile publication and all nine fiducials, not merely a PipeWire stream
  state transition.
