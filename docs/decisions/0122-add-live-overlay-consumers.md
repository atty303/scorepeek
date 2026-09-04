# ADR 0122: Add independent native and OBS live overlays

- Status: Accepted
- Date: 2026-09-04
- Supersedes: the UI exclusion in ADRs 0119 and 0120 and the roadmap.

## Decision

The only distributed executable remains `scorepeek`. `run --overlay wayland --overlay obs`
starts independently optional same-executable children; omission starts neither. Wayland output
selection is `--overlay-output NAME` (exact name, required for multiple outputs). OBS listens on
`--overlay-listen IP:PORT`, default `127.0.0.1:17384`, loopback only. The private child entrypoint
does not initialize OCR or capture. It receives the parent's resolved socket, invocation, absolute
score DB path and display options through stdin. EOF ends the child. The parent closes all leases,
waits up to two seconds, then terminates and reaps only children it owns. There is no restart loop.
An overlay failure is visible in run status but does not stop recognition, persistence or its peer.

`scorepeek-overlay` owns transport, display projection, native shell and the local Web server.
`scorepeek-overlay-ui` contains shared Dioxus presentation types/components and CSS, without
recognition or SQLite. `scorepeek-overlay-web` owns browser WebSocket reception and coalesces
latest state into at most one Dioxus update per requestAnimationFrame. The handles adapter owns
the SCTK shell and raw Wayland handle borrowing; destroy-capable proxies never escape its private
surface owner. GPU handles retain that owner even after the shell drops. Production never imports
the isolated spike workspace.

### Authority and presentation

Consume public v1 snapshot/live NDJSON only. Reconnect on EOF or sequence gaps and replace state
from snapshot, preserving notification identity rather than counting retained results as new plays.
Retained results restore confirmation independently of Live. A snapshot after session end cannot
restore old Live content. An unresolved selection can retain an already observed chart only when
its event identity and capture still match; a missed selection cannot be reconstructed from that
slot, so the overlay waits for the next confirmed selection instead of guessing a chart/history.
No handshake, new public event, PLAYING detector, attempt notification or history API is introduced.
Recognition, RESULT confirmation and score persistence retain their existing authority.

Three translucent, unblurred cards sit at the right of the display:

- Live retains confirmed selection through play. Evidence-unresolved selection dims previous
  content with 選曲確認中; episode end does not invent PLAYING. Resolved provisional RESULT shows
  available performance, judgments, timing and options; revisions update it and withdrawal removes
  it. Unknown values are not zero. SELECT best shows only EX/MISS/CLEAR receipt; explicit no-record
  is received, not a saved-play or DB-commit indicator.
- Latest play shows title and 確定待ち / 確定 ✓. Confirmation is `result_detected`, correlated by
  capture/attempt identity, never a title match or a DB transaction. It survives selection until
  the next provisional result replaces it; no previous-unconfirmed warning accumulates.
- Selected-chart best/history uses the common `scorepeek-scores` read-only query, latest five
  saved results and per-field integrated best. Dates are UTC notification timestamps, not play
  start times. Keep the selected chart through play/result, switch on next confirmed selection.
  Query on chart change and each second, publishing only content changes. A DB failure retains
  same-chart data marked 更新停止; it cannot relabel another chart's history. Readers neither
  create nor migrate a DB. `--no-scores` disables reads as well as writes, without a fallback DB.

Disconnect dims retained Live content with 接続待ち; history polling continues. The browser receives
only latest display snapshots, not diagnostics or raw public events.

### Rendering and distribution

Use Dioxus 0.7.10 and Blitz revision `64eb27853aa2672486b7edf825fb044be78c9db3`, native DOM,
AnyRender/Vello and SCTK/calloop. The child selects Vulkan at runtime; the upstream dependency
graph still compiles other GPU backends. No Winit, GTK, WebKit, CPU renderer or upstream patch is
introduced. System fonts use Fontique/fontconfig; native and browser are not pixel-exact.
Layer-shell uses overlay layer, all anchors, transparent background, empty input region and no
keyboard interaction. Fractional scale uses viewporter when both protocols exist; otherwise use
integer scale. Dioxus wakes calloop and CSS animation requests Wayland frame callbacks only while
active. No idle paint loop is installed.

Axum serves `/`, dx-generated assets and `/ws`. Pinned `dx bundle --web --release` generates assets
that `rust-embed 8.12` includes in the one executable. Normal checks do not require a bundle;
distribution and Web acceptance do. The moved-binary test uses the real bundle, not placeholders.
The dx-generated JS/WASM is browser-only; Node.js and dx are not runtime dependencies.

Private child diagnostics are distinct from UI/WebSocket data and flow to the existing opt-in
run-event recorder as `overlay_observed`. They contain operation summaries/counts and errors, not
screen contents or credentials. They do not extend public v1. Renderer call counts describe
submission attempts; AnyRender's unit-returning render API does not certify compositor display.
Visual presentation, GPU behavior and input passthrough require the explicit live gate.

## Verification and execution boundary

Implement directly on main, preserving unrelated work and retaining the spike worktree only as
reference. Require format/lint, shared WASM check, display transition and read-only DB tests,
child/real-asset tests, regression tests and fresh review before committing. No push, deployment,
autostart or release is authorized by this decision.

`mise run overlay:test:live --scores-db PATH` requires a dedicated instance. Observe selection,
provisional revision/withdrawal, confirmation, next selection, native/OBS agreement, history,
Japanese fonts, scaling, input passthrough, Gamescope fullscreen, idle rendering and cleanup.
Record CPU/GPU and OBS rendering lag separately. Development-host success is not target-machine
support. Current verified and unverified boundaries live in `STATUS.md`.
