# scorepeek committed checkpoint

This file describes only the state included in its commit. Uncommitted changes are outside the
checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver/Gamescope observed-frame profile and M4 canonical recognition,
  evidence-first attempt resolution, and versioned event API remain in progress.
- RESULT payload v2 is shared by provisional, retracted, and confirmed attempt state. MUSIC SELECT best snapshot v1 is a
  separate supplemental observation, not a play. ADR 0120 adds local score persistence;
  query CLI remains outside scope. ADR 0122 adds independent Wayland/OBS live overlays.
- The public live API is Event API/socket snapshot v2 on stable `events.sock`. Diagnostic protocols remain run-event v12 and recognition observation v22.
  Joined sessions and private attempt labels remain v5. Readers retain supported older shapes and
  reject unknown versions.

## Implemented authority

- ADR 0143 replaces the binary-owned skin set with local, user-installable ZIP packages. The
  package boundary requires a manifest, one core Wasm module, CSS and PNG preview; optional WebM
  and package-relative resources remain opaque. `scorepeek skin install`, `uninstall` and `list`
  manage original ZIPs in the XDG data store. Install performs structural, PNG, ABI and two-backend
  smoke validation against a scorepeek-owned fixed staging snapshot before atomic replacement;
  same ID/release is a no-op. Native executes the shared JSON ABI in Wasmtime with an epoch hard
  timeout covering instantiation and every guest export and reconciles keyed ordinary DOM through
  Blitz. OBS executes one instance per canvas in a Web Worker and reconciles ordinary browser DOM
  under external package CSS/runtime routes and a same-origin CSP. Mandatory PNG and optional WebM
  previews remain package validation metadata but are not decoded into the shared editor panel; the
  selected canvas itself is the live preview. The native editor renders the selected
  draft through its package runtime rather than the superseded fixed renderer. No registry,
  digest/signature, quota, fallback,
  interaction API, WASI/host import, or running-instance replacement protection exists.
- Overlay TOML is schema v7. Skin identity is an arbitrary reverse-domain ID and canvas/widget
  properties are persisted maps resolved through installed manifest schemas. Schema v5 maps the
  three old names to formal IDs and preserves background/frame/fill values. Schema v7 removes all
  persisted overlay revisions and initial placement, permits zero canvases, and requires an output
  ID for every canvas. A missing initial config creates empty Wayland and OBS workspaces; the visual
  editor is the creation path. No installed skin is a startup error. The repository retains a Rust
  guest SDK/core and three package sources; their shared guest recreates the full status, selection,
  score, history and graph DOM, material frames, atlases and semantic styling from the superseded
  fixed renderer.
  `mise run overlay:skins:build` scopes package CSS below the noninteractive skin root and produces
  uncommitted ZIPs without installing them.

- Repository skill `.agents/skills/create-overlay-skin/SKILL.md` defines new-skin and requested
  redesign work from concept comparison through package Wasm/CSS implementation and native/browser visual
  evaluation. Its references separate IIDX facts, current display contracts and approved design
  choices. This adds no runtime skin or changes to existing skin assets.

- ADR 0144 makes Wayland and OBS adapters of one backend-neutral editor session, reducer, Dioxus DOM
  and CSS. Wayland and OBS keep independent workspaces, while the shared hierarchy is workspace,
  output, canvas and widget; game-screen preview is an orthogonal six-value state including UNKNOWN.
  Omitted `show_on` means all contexts and explicit lists remain explicit. Zero canvases is valid,
  and the empty editor retains output and skin selection before CREATE FIRST CANVAS. New canvases
  fill the active output's exact logical outer bounds and start in only the current preview context.
  Canvas movement uses secondary-button drag; widget selection, movement and resize use the primary
  button. Output reassignment and FIT TO OUTPUT are explicit undoable operations.
  Outside editing, Wayland owns one surface per canvas; editing temporarily supplies one full-output
  stage per connected output, with only the active stage accepting management input. Empty Wayland
  startup opens an internal full-output editor stage automatically. OBS always uses one full-output
  stage, with right-click on blank space opening the same editor. A single parent writer serializes
  backend-local leases and atomic commits against one startup-loaded document; Wayland and OBS may
  edit concurrently. A process-lifetime same-path lock rejects a second writer. External edits are
  outside the contract, while renderer projection generations remain run-local transport state.
  The Wayland rasterization cap remains a runtime setting outside the shared canvas editor transaction.
  Installed skin packages remain canvas-owned and immutable for each running projection.
- ADR 0141 makes the Gamescope receiver always request a 1920x1080-bounded capture domain. Gamescope
  can preserve aspect ratio, so the negotiated observed dimensions remain authoritative. Its
  startup full-output contract and corrupted transition buffer may be replaced only before the
  first valid frame; later drift fails closed. Every producer buffer is still returned promptly,
  while application-owned full-frame copies follow fixed 10 Hz deadlines. Shutdown deactivates and
  quiesces the stream, services a bounded in-flight grace period, then disconnects. Physical output
  and observed capture dimensions are independent calibration facts. Existing full-size profiles
  therefore fail admission and require `scorepeek setup gamescope`; they are not rewritten.
- ADR 0142 keeps the setup-owned static X11 calibration marker repainting one unchanged pixel at
  10 Hz. This bounded damage heartbeat lets Gamescope produce a valid frame after applying
  `requested_size` without changing marker pixels, fiducials, the common receiver or ordinary game
  capture.
- ADR 0139 makes the status widget RESULT lamp follow the explicit result state: inactive is unlit,
  provisional/confirmed is green, and retracted is red. PLAY and capture-session start publish
  inactive; session finish retains the last result state.
- ADR 0136 makes the native Wayland shell select the C client backend and dynamic loading
  explicitly. Building no longer requires host Wayland pkg-config metadata, development headers or
  an unversioned linker name; `libwayland-client.so.0` remains a live-runtime host boundary.
- CYAN SYSTEM, RESULT AURORA and DJ BLACKBOX remain repository package sources rather than
  executable-owned skin variants. Their prior fixed Dioxus renderer and embedded-asset authority
  below is superseded by ADR 0143's Wasm/full-tree/package-resource contract. They previously used shared image-backed frames
  with fixed-aspect corners (ADR 0134), SVG chart/status fittings and embedded surface artwork. Oxanium and OFL 1.1 are embedded alongside Japanese system-font
  fallbacks. ADR 0131 adds embedded Orbitron/Rajdhani, new transparent
  energy/circuit artwork and semantic clear/difficulty/rank treatments. Shared motion tracks drive
  native Blitz styles and browser JS continuously while visible, without changing actual values.
  Decorations overflow widgets and are cropped by the canvas; hidden native surfaces remain idle.
  ADR 0133 restores the original design sheets as the visual reference, with compact new-widget
  proportions, chamfered contours and restrained semantic motion. The chart rail, badge and lamps
  have explicit shared geometry; history graphs show parent-generated local-calendar month labels.
  Saved geometry is not rearranged.
  EX SCORE and DJ LEVEL use skin-specific embedded material glyph atlases from bundled fonts.
  Headings, chart labels, judgments, clear types, history columns and graph labels share proportional
  material label atlases; mixed-language song titles and dynamic small values use ordinary text.
  Actual values remain accessible DOM text.
  Japanese uses system fonts without an embedded Noto bundle (ADR 0132).
- Score/history widgets read only committed SQLite state. BEST integrates RESULT and SELECT sources;
  representative RESULT ordering is highest EX score, known/lower miss, then latest receipt time.
  History rows include DJ LEVEL. The graph uses exact timestamps, labeled DJ LEVEL thresholds and a
  fixed 0-100% MISS RATE axis, clipping larger MISS ratios and leaving unknown values disconnected.
  Successful SELECT/RESULT score transactions publish a live-only `score_store_changed` chart
  invalidation, so a matching overlay rereads SQLite immediately. The five-second poll remains a
  recovery path for external writes or missed notifications.

- RESULT temporal acceptance compares the mandatory song/chart, clear, EX and judgment tuple.
  Supplemental/reference changes do not revoke it (ADR 0083/0087); once accepted, repeated
  supplemental payloads may update the presentation without blocking close-time confirmation.

- Event API v2 removes `result_ingest_changed`; recognition state is the single `result_changed`
  lifecycle, successful DB commits publish `score_store_changed`, and persistence failure remains in
  score health/status. Unknown additive v2 events are ignored only after envelope/sequence validation.
- Overlay consumers still do not initialize recognition or own capture resources. Children receive
  invocation/socket/DB/config and terminate on parent-pipe EOF; one overlay failure does not stop
  recognition, persistence or its peer. Parent controller diagnostics use a bounded in-process queue
  and the recording path rather than writing JSON to the TUI terminal. No overlay flag preserves the
  overlay-free behavior.

- ADR 0139 advances `scorepeek-scores` to the independent public Event API v2 consumer. Normal run saves to
  the XDG data score database; `--scores-db` selects an instance and `--no-scores` disables it.
  Provisional RESULTs insert/update one attempt row immediately, retraction deletes it, and confirmation updates it. SELECT-only charts have best rows without plays;
  SELECT retains per-field current supplements, not revision history. Later known/no-record values
  can correct supplements, while RESULT/previous-best sources retain cumulative bests.
- Integrated chart bests retain per-field provenance and are recomputed after RESULT retraction as well as SELECT corrections.
  Database schema v2 migrates existing plays as confirmed, preserves the first provisional timestamp
  across re-resolution, and promotes crash-left provisional rows to confirmed with recovery provenance
  only after acquiring the database-specific single-writer lifetime lock.
  Public events carry an immutable `emitted_unix_ms` notification timestamp. SQLite transactions,
  WAL/FULL, bounded worker admission and bounded drain separate committed from unsaved data.
  Socket failures no longer freeze public projection or score delivery. Save failures stop score
  admission without changing recognition; run status and opt-in diagnostic health show degradation.

- The registered PP-OCRv6-small and private fixed-cell HOG/MLP bundles remain the only text/numeric
  runtimes. Capture is canonical contiguous RGB8 1920x1080. Gamescope PLAY uses the independently
  measured BPM-outline screen-path layout v4 (ADR 0121), covering both SP graph positions; SELECT
  badges use the two explicitly approved crops.
- ADR 0117 replaces SELECT difficulty RGB area counts with the independently measured PLAYER 01
  outline in integrated-context layout v6. Both thin white edges must contrast with their interior;
  a single slot must meet 80% coverage and a 10-point winner margin. No extra model, template bitmap,
  dependency or temporal voting is introduced. Raw difficulty remains separate from best values.
- Raw 10 Hz screen observations and semantic episodes are separate. UNKNOWN suspends, matching
  known screens resume, and transitions drain admitted work before RESULT finalization. Shared OCR
  workers commit observations in source order. Identity uses independent song/chart factors,
  normalized family support and separate song/sibling-chart margins. SELECT and RESULT retain
  independent resolvers; best values never become identity evidence.
- `result_changed` emits inactive/provisional/retracted/confirmed around the unchanged
  `scorepeek-result-detected-v2` payload; only confirmed increases result count. Ordered optional play options
  use the fixed label/marker and two matching observations. SELECT incumbent/successor evidence and
  latest-known difficulty hand off to the attempt after close-time drain.
- ADR 0114 adds the independently measured SELECT SCORE DATA layout v1. SCORE, MISS COUNT and clear
  type use the existing runtimes. Neutral-bright masking excludes dim leading placeholders; the
  fixed 1.0 numeric logit margin fails closed. Four measured dashes are explicit no recorded MISS;
  missing header or inconclusive OCR remains unknown. DJ rank is derived from chart notes/EX SCORE.
- ADR 0115 registers the SELECT-adapted HOG/MLP weights through runtime manifest artifact v3.
  The architecture, runtime schema v2, layouts and all acceptance thresholds are unchanged.
  Digest-bound provenance identifies the parent, private supervision, retention teacher and recipe.
- Best fields need two equal consecutive observations independently. Partial snapshots are allowed.
  Current-frame song/mode/difficulty must agree with resolved identity. ADR 0118 retains the
  interval and last publication across missing evidence while resetting field streaks and stopping
  adoption. Contrary credible song/mode/difficulty evidence ends the interval and clears best. Duplicate/reversed or pre-resume frames cannot update best; suspension retains publication but resets field streaks,
  closing blocks supplemental emission, and SELECT exit clears it. First and changed content emit;
  revisit starts a new observation. No achievement date, play count, option or common-play relation
  is inferred. Admitted identity evidence still drains during suspension/close; supplemental
  suppression never discards identity evidence.
- Resolver notifications compare semantic state: resolved clock/streak updates alone do not emit.
  Internal observations remain fresh; connecting-client snapshots use the last published state.
  Held identity, current stabilization and the last published revision are distinct in the TUI.
- ADR 0139 promotes stable `events.sock` and Event API v2 as the public snapshot/live NDJSON API. One four-state RESULT,
  current selection, supplemental SELECT best and operational status have a separate typed projection,
  public sequence, event identity and session binding. Raw observations, candidates, resolver state,
  timing, recording paths and history arrays remain internal. The observation socket is removed;
  TUI, run-event v12 recording and headless replay use the same result-state vocabulary.
- Snapshot and publication share the sequence boundary. Queue overflow disconnects existing clients;
  slow clients are isolated. Events and snapshots are bounded to 1 MiB. Oversize or worker failure
  disables public delivery without changing recognition. Reconnect restores current state, not all
  missed plays. Detailed wire and consumer state rules are in `docs/event-api.md`.
- TUI has Watcher, Latest result, Music Select Resolver, and RESULT/attempt Resolver. At 80x25
  the four panes occupy 4/8/7/6 rows and retain all attempt gates.
- ADR 0116 restricts recording completeness to runtime persistence loss. Typed facts are written
  without duplicate semantic validation; internal binding/shape/chronology defects are no longer
  emitted as recording-drop reasons. Foreign pending jobs are rejected without degrading the run.
  Existing historical reason names remain readable and stored session completeness is unchanged.
- `--record` remains opt-in for capture/recognition/events/canonical artifacts. Without it, live
  state and domain behavior operate without recording. Runtime recording loss changes completeness only.
  Canonical retained frames are lossless RGB Matroska segments with a shared memory account and
  typed intentional gaps. Successful joined publication clears owned staging; failures retain
  diagnostics. No live target, remote storage policy, or external service was changed here.
- Private corpus import uses immutable local metadata and optional S3 segments, digest-bound ranged
  downloads, bounded shared replay workers/memory, production recognition/reducers and the existing
  accepted-result oracle. Supplemental snapshot counts are reported separately per session. Optional `--trace-dir` saves
  state/domain events (excluding raw field candidates), with a shared 256 MiB budget, no overwrite,
  code/model/layout binding and non-interfering recording failure status.

## Verification

- The three repository skin ZIPs build reproducibly enough for local install testing and contain the
  mandatory manifest, Wasm, CSS and preview plus package-owned artwork, font and license resources.
  Isolated-XDG CLI trials cover install, identifier-ordered list, same-release no-op and uninstall.
  Workspace tests cover manifest/identity/property migration, package resources and owned child
  shutdown. The native 1920x1080 visual fixture completed all 28 editor, skin-switch, motion,
  geometry and screen captures with selector layouts; all three skins retain full widget content and
  load their own frame resources after a draft switch, while selection, movement and resize remain
  operable over the noninteractive skin tree. A production OBS page initialized and rendered the
  same package Wasm with an idle next tick, and the full embedded-web integration passes with the
  complete package resource set. The shared Wasm guest now preserves the original Dioxus widget
  surface contract: the saved rectangle stays unchanged while every populated renderer receives
  the 16-pixel-expanded surface behind its negative-eight-pixel origin. The status frame is therefore
  560x60 for a 544x44 saved rectangle in both native and browser DOMs, with its wordmark and two lamps.
  The separate 21-capture composition scenario verifies package
  background subtraction through four empty apertures, frame/background plugin property controls,
  title editing and aspect changes. A production 1920x1080 Codex Browser run found that the Wasm
  canvas CSP rejected plugin-declared inline geometry and that the replacement runtime dropped the
  editor's sample-state request. The browser host now applies plugin style attributes through the
  CSS object model without weakening the same-origin CSP, and forwards sample mode to the state socket.
  The same run confirmed all five populated widgets at their saved rectangles with full sample data,
  material frames and backgrounds for Cyan System, DJ Blackbox and Result Aurora. These are
  development-host browser checks; live Wayland compositor and actual OBS Browser Source rendering
  remain separate boundaries.
  Native editor pointer delivery no longer treats a readonly backend lease as a reason to discard
  every button event: shared navigation remains available while the editor model still rejects
  draft mutations. The first complete post-open pointer cycle emits one bounded, value-free
  `native_editor_pointer` observation with dispatch and handler counts. That observation and a
  Wayland protocol trace proved the remaining failure occurred after a Close action reached Dioxus:
  Blitz incremental layout retained a removed layout-child ID and panicked while resolving the next
  paint. A Dioxus mutation now forces one full layout before restoring incremental animation paints.
  An automated nested-Scroll run on the development host verified Close, right-click reopen,
  MODE SELECT state change, a second Close and clean process shutdown. The focused readonly model
  regression, complete repository suite and 28-step native visual scenario pass. This is live
  Wayland protocol, composition and input evidence in a nested compositor, not target-compositor or
  target-game-session validation.
  The native drag-performance regression covers 100 skin update requests coalescing to one frame
  render, immediate release flushing, and canvas x/y changes avoiding skin invalidation while canvas
  size and widget geometry still invalidate it. A retained native tree also restores the live skin
  CSS after a different preview skin. The 80 overlay library tests and a fresh 28-step native visual
  run pass after the package-retention change. Target-compositor performance for sustained
  widget move/resize remains a separate live verification boundary.

- The repository skin skill passes structural validation, scoped checks and independent review.
  Forward trials cover concept generation/correction, missing required tools, selection gates and
  an unrelated connection-diagnosis request. A real 1920x1080 Codex Browser trial exercised Aurora
  selection, widget resize, frame change, save/reload and discard in an isolated configuration;
  native inspection covered the 22-step fixture and five Aurora captures including motion times.
  This verifies sampled skill stages, not a complete new skin or the full per-skin matrix. Generated
  comparison images with inconsistent content were not accepted as implementation references.

- Gamescope `3.16.19-128-g7282613+` on the development host reproduces the original pressure at
  5120x1440@120 Hz: before ADR 0141 it supplied about 120 full BGRx frames per second, and a bounded
  vkcube consumer disconnect reproduced Gamescope's `destroy_buffer` assertion. With 10 Hz copy
  admission, `requested_size` and quiesced shutdown, the final 10-second run negotiated 1920x540
  BGRx MemFd with 7,680-byte stride from that ultrawide source and copied 101 frames. Ten subsequent
  one-second lifecycle runs each copied 11 frames, completed receiver/provider shutdown, retained
  bounded file descriptor/thread/RSS counts and left Gamescope live. Its observed log contained
  streaming/paused transitions without `out of buffers` or a PipeWire assertion during this gate.
- The same isolated `scorepeek setup gamescope` command failed before ADR 0142 with
  `FirstFrameTimedOut` after Gamescope's requested-size stream transitions. With the unchanged-pixel
  heartbeat it published a bounded observed profile and verified all nine fiducials. The windowed
  3840x2160 development-host run observed 1920x1074 because its actual source content was not exact
  fullscreen 16:9; this proves setup completion, not the target fullscreen geometry.
- In one subsequent operator-observed INFINITAS target session, Gamescope's `out of buffers`
  warnings decreased substantially after the bounded-capture changes, and Gamescope remained alive
  when scorepeek exited.
- A separate monitored target session confirmed the published `gamescope-4k` profile and the live
  Gamescope-to-scorepeek PipeWire contract as BGRx 1920x1080 with the private requested size also
  1920x1080. Both nodes remained running with zero PipeWire `ERR` across 31 samples. Scorepeek's
  public status remained `session_active` with generation 1 and ready catalog, model and score store
  after Gamescope had run for at least 958 seconds. Gamescope console output was not retained in the
  journal; the operator independently observed about ten `out of buffers` lines over the session.
  After the operator ended it, neither process remained, and the retained coredump and system
  journal records contained no scorepeek or Gamescope crash, assertion or `destroy_buffer` entry.
  This completes one of the three 15-minute target runs, but not the repeated lifecycle or 30-minute
  soak gates.
- During that monitored run, Gamescope stayed at 114,256 KiB RSS and 14 threads, while the main
  scorepeek process grew from 1,229,276 KiB RSS at 205 seconds to 2,505,356 KiB at 972 seconds with
  24 threads. The measured growth was private anonymous memory with no swap. It sometimes slowed
  substantially but had not established a bound before shutdown, so it remains a soak risk rather
  than a confirmed unbounded leak.

- The Wayland shell passes its standalone locked build, link and test gate through the repository pkg-config
  boundary, which exposes only the pinned PipeWire SDK. The same check passes from an empty Cargo
  target directory and a clean `cargo check --locked -p scorepeek` combines it with the pinned
  PipeWire/native build inputs. Neither check uses feature unification from another workspace member
  or a host `wayland-client.pc`.

- Overlay visual debugging has three reproducible development-host entries. The native scenario
  runner keeps one Dioxus editor workspace across selector clicks, Blitz scrolls and logical-pixel
  drags plus runtime screen changes, retains one image renderer for the complete scenario, renders
  every step through native DOM/Blitz/Vello without a Wayland connection, and writes
  correlated PNG, selector-layout JSON and a complete/partial typed manifest. The OBS runner serves
  the production `/overlay`, canvas iframe, WebSocket and editor from a create-only dedicated config
  on loopback without capture, recognition, Wayland or a score DB. The checked-in 1920x1080 scenario completes through
  panel toggles, tab selection, scroll and right-drag; Codex Browser opens the runner URL, enters the
  editor by right-click, reads the iframe DOM and captures the composed image. This visual path
  exposes native paint/CSS differences that positive DOM rectangles alone do not establish. Pixel
  equality is intentionally not an acceptance gate; actual Wayland composition/input delivery and
  rendering inside OBS remain live boundaries. A 1.25-scale headless run records 800x600 logical
  geometry and emits 1000x750 PNGs; invalid scenario dimensions are retained as `scenario_invalid`.
  The checked-in native scenario also switches a RESULT canvas through three hidden screens and
  back to RESULT on that same renderer, so image-resource loss across normal canvas visibility is
  represented instead of being hidden by a fresh renderer per capture.
  The Wayland rate contract has deterministic AUTO/cap/bypass and strict schema-v5 migration tests.
  The representative 28-frame native scenario and the 20-frame composition scenario complete with
  selector layouts; the latter shows all four empty-aperture geometry overlays at their logical
  rectangles. A fresh 1920x1080 Codex Browser run against production `/overlay` starts from no OBS
  canvas, opens the editor by right-click, selects UNKNOWN and DJ BLACKBOX, creates an exact
  1920x1080 canvas, adds a status widget, deletes the last canvas, restores it with UNDO, saves and
  returns to display-only rendering. The canvas iframe now keeps its Wasm instance across same-skin
  move, resize and property changes and receives presentation differences over its existing
  WebSocket; canvas lifecycle, skin/sample changes and editor close/reopen remain reload boundaries.
  A follow-up Browser run covered two canvases, UNKNOWN preview, widget placement and drag, canvas and
  widget properties, undo, save/reopen and discard/reopen without changing the iframe URL during
  same-skin edits. The browser reports no warning or error. The checked-in Wayland runner starts
  the production native child against an isolated config. Inside a real nested Scroll compositor, an
  empty workspace promoted its bootstrap stage to the discovered WL-1 1716x1494 output; compositor
  pointer input selected UNKNOWN and DJ BLACKBOX, created and saved a canvas with exact 1716x1494
  bounds, and the child reported complete shutdown after the bounded run. A follow-up run deleted
  that last canvas, saved `canvases = []`, and confirmed that the empty editor stage was recreated in
  the same child process instead of leaving the workspace unreachable. This is nested-compositor
  protocol, composition and input evidence; actual OBS Browser Source and a non-nested target
  compositor remain separate boundaries.
  A two-output nested Scroll regression run keeps one stable full-output editor stage on each
  1716x1494 output while a canvas is created and reassigned from WL-1 to WL-2. The reassignment causes
  no surface recreation, configure timeout or worker failure. Editor skin motion is paused, unchanged
  keepalive responses do not touch reactive state, and both stages stop painting when idle; the final
  summaries recorded 6 and 5 paints over about 170 seconds instead of the pre-fix run's roughly 2200
  paints per stage. UNKNOWN preview, opacity change and output navigation were also exercised in
  that bounded run. A separate
  passive-stage check clicked WL-2 while WL-1 was active: no editor pointer event was emitted and
  Scroll handled the input. Transition tests also keep a runtime-hidden stage input-disabled when
  the editor closes.
  Timestamped child diagnostics now correlate startup phases and editor input-to-paint work by
  process, run and interaction IDs, including control, skin, Dioxus, renderer-wait and paint time.
  In a three-canvas/two-output nested-Scroll reproduction, the pre-fix first editor paint took about
  12.4 seconds and a cross-skin canvas selection took 2.667 seconds: 2.260 seconds were repeated
  Wasmtime compilation and 351 ms were shared-editor reconstruction. The native runtime now keeps a
  bounded on-demand exact-byte compiled-module cache with per-store deadlines on a shared periodic
  epoch clock, skips the discarded full bootstrap
  application when outputs are discoverable, and does not initialize a selected preview on a
  surface that does not own that canvas. Removing package
  thumbnails from the shared panel reduced its reconstruction to 1.85 ms. The same nested llvmpipe
  route then reached first paint in 3.87 seconds and painted the canvas selection in 292 ms (0.61 ms
  state, 228 ms instance/tree, 1.85 ms Dioxus and 61 ms paint). A later cold one-output run attributed
  2.038 of its 5.149 seconds to module compilation and 1.604 seconds to the llvmpipe first paint;
  cache hits in the same process took 18 microseconds. Output reassignment painted on the
  source surface in 43.8 ms and initialized the destination skin once without a repaint loop. A
  Chrome run of production `/overlay` right-clicked into the empty OBS editor, added a canvas and
  changed opacity in 1.3 ms while retaining the exact canvas iframe URL. A fresh 28-step native
  PNG/layout/manifest run remained complete. These are development-host measurements, not target
  compositor, GPU or OBS Browser Source performance gates.

- Schema-v2 defaults/rejection, screen filters, semantic-screen snapshot/live folding,
  suspension/disconnect grace and immediate known-screen replacement have focused development-host
  tests. Workspace compilation covers the Wayland cursor-shape and generated fallback cursor,
  empty input regions, shared feed, content opacity, preview lease transfer and OBS stage routes.
  Current native target screenshots confirm one-to-one preview and stable panel/footer layout on the
  5120x1440 integer-scale output and both rotated 3840x2160 fractional-scale outputs. A headless
  1920x1080 Chromium screenshot confirms the same `/overlay` editor composition and clean initial
  draft state. Each overlay process was stopped immediately after its screenshot. Pointer gestures,
  native output handoff, compositor cursor behavior and real OBS Interaction remain unverified.
  On the current three-output Wayland host, editor startup with four configured canvases and
  editor-only peer surfaces now keeps Vulkan context, adapter, device and surface initialization
  serialized inside the single backend child process. Renderer resume, resize, paint, suspend and
  destruction are serialized across those surface workers. Each output keeps one deterministic
  editor host surface for the lifetime of the workspace instead of transferring the editor when
  canvas selection changes, and unchanged geometry does not issue another layer-surface commit.
  The child remained live with all surface workers present, stopped cleanly, and produced no new
  coredump. A live right-drag and confirmation on this host produced neither compositor animation
  nor missing canvas background; this is one target-host confirmation, not a general compositor
  compatibility claim.
  A later target play session reproduced missing PNG widget backgrounds after semantic-screen canvas
  switching. The same image-present, image-absent, image-present sequence now reproduces headlessly:
  Vello 0.10 replaces its persistent image atlas on the image-free frame while retaining stale image
  residency metadata, so the returning PNGs are not uploaded. Native paint now keeps an invisible
  image patch in every frame to keep the atlas generation and residency metadata synchronized, while
  hidden surfaces still stop repainting after their one transparent transition. The persistent-renderer
  scenario retains all PNG backgrounds after the three hidden screens. Target installation and live
  confirmation of this repair remain outstanding.

- Overlay development-host verification covers all three skin DOMs, embedded PNG decode, fixed
  widget bounds and explicit motion time samples. A production native headless render confirms Japanese/Latin
  text, selection rail, DB-derived BEST/DETAIL, DJ LEVEL history and graph dots/thresholds. Strict
  TOML, missing-file creation, invalid-canvas isolation, atomic save, lease/revision conflict,
  backend canvas management, local-time formatting and readback triggers have focused tests.
  The browser WASM type-checks and the real dx bundle contains served JS/WASM/font/artwork with correct
  MIME types; embedded-asset and child-EOF tests pass. `wasm-opt` still reports unsupported DWARF and
  the bundle proceeds without that optional optimization.
- The skin-specific typography and proportional label atlases have all-three-skin native image
  inspection and a complete 22-frame native PNG/layout/manifest scenario, including hidden-screen
  transitions and editor resize/scroll/drag. Atlas labels align with adjacent ordinary text using
  font baseline metrics; chart rails, detail rows and history rows have all-three-skin native inspection.
  The current typography also has development-host Codex Browser inspection through the editor
  and canvas iframe; live OBS composition remains separate.
- ADR 0131's three rich skins have native PNG/layout/manifest inspection and 1920x1080 Codex Browser
  inspection through the production editor and canvas iframes. Native samples at 2.25 and 3.75 seconds
  keep widget/value geometry fixed while light and particles move; inactive/active/error lamp paint
  remains distinct. Blitz SVG support renders graph polylines using explicit paint attributes and
  a viewport shared with plot geometry; colors are shared with CSS axes and legends. Frames use
  restrained lines/corner accents, with staggered moving highlights carrying the ambient motion.
  These checks do not establish live Wayland or OBS composition.
- Repository checks and the complete workspace suite pass: 510 library,
  324 binary, 128 corpus library, 5 corpus binary, 87 overlay, 7 handle, 14 overlay-UI, 7 overlay-web
  and 20 score tests, plus doctests. The embedded-web overlay integration test also passes. The 99 offline OCR tests and
  repository checks also pass. Public API and overlay state tests include score-store invalidation,
  RESULT readiness across withdrawal/re-resolution, fresh and same-session reconnect restoration,
  and completion publication after shutdown drain.
  The supplementary all-features/all-targets pedantic Clippy invocation also passes.
- Target investigation found two connected outputs while the initial Wayland canvas omitted
  `output`; the previous child rejected that multi-output state before creating a surface. The
  native child now selects the first named connected output in stable name order when `output` is
  absent or stale, persists the resolved name through the parent, retries transient save conflicts,
  and releases an active editor lease when its surface exits. Focused fallback tests, workspace
  all-target Clippy and the complete suite pass on the development host.
- Target use of the prior build established that native surfaces, render workers and Vulkan were
  active while every widget was visually absent. The shared intermediate DOM wrapper had no definite
  height; its absolutely positioned widget descendants did not contribute layout height, and the
  nested canvas clipped them at zero height. Commit `d0d510f94179afd55ff6b6dde74e93af0d44f158`
  gives that wrapper full-canvas geometry in both Wayland and OBS and adds a 560x72 regression that
  checks the wrapper, canvas and status widget bounds. The complete development-host suite passes.
- Commit `d0d510f94179afd55ff6b6dde74e93af0d44f158` was built with `mise run dist:test` and
  installed on `infinitas.lan` as `/home/atty/.local/bin/scorepeek`. The installed binary SHA-256
  is `2dfd53f2710ca902e3cc6300a3735d9ad3bf530aa53f6d90bad0bd78c7a047e3`; target readback verified
  the hash and version, and same-directory staging and rollback files were removed after atomic
  replacement. No scorepeek run or overlay child remained at post-install inspection, so the next
  run will load this binary. Target visual confirmation of the repaired widgets remains outstanding.
- Commit `00a04b17a7f615310866c7eec47efb5421b7c583` was built with `mise run dist:test` and
  installed on `infinitas.lan` as `/home/atty/.local/bin/scorepeek`. The installed binary SHA-256
  is `1379f95d2a5cba9b2fc262c01c226e8ef9759643a51806366c02c9b00528b1a8`; target readback verified
  the hash, version, mode `0755`, and removal of the same-directory staging and rollback files after
  atomic replacement. No scorepeek process remained at post-install inspection.

- Recorded-input reducer replay of the complete session with digest
  `193550c1c3337905122585fb868c1c8831be3fab835c5ec9e5c03ef70c419594` confirms four results,
  matching the four private visual labels. Restoring the old full-performance equality alone
  reproduces three results: a final MISS `not_displayed` to `unknown` observation revokes the first.
  The other three payloads are unchanged. This reuses recorded field/semantic inputs through the
  production reducer; it is not fresh OCR, full corpus replay, target installation or live validation.
  Candidate labels remain unapplied and no score database is backfilled.
  All 504 library tests, library-only Clippy and the focused reducer tests pass.
  Root check/test currently stop in unrelated, uncommitted overlay
  formatting changes; this checkpoint does not claim an all-workspace validation pass for this fix.

- ADR 0121 production predicate evaluation covers 89 retained frames from the SP graph-position
  failure session. The inspected left-graph PLAY frame changes from UNKNOWN to PLAY; the other
  88 classifications remain unchanged (including 77 known non-PLAY controls and two PLAY frames).
  Synthetic regression covers both independently measured SP positions and rejects solid cyan
  panels at both positions. Thresholds and the exactly-one-screen gate are unchanged.
  A separate 786-frame legacy QOI comparison against the installed v3 build has zero screen
  classification changes: 41 PLAY, 182 RESULT, 75 SELECT, 30 DECIDE, four MODE and 454 UNKNOWN.
  An independently inspected DP PLAY frame also retains its classification and all outline metrics.
  `mise run check`, the complete `mise run test` and independent review pass for this fix.

- Scores tests cover production projection to SQLite readback, SELECT-only charts, later RESULT,
  downward SELECT corrections, partial/no-record fields, source ties, chart/instance separation,
  transaction rollback, schema mismatch, concurrent database initialization, bounded locks, queue
  overflow and shutdown timeout.
  Socket worker loss and database initialization failure do not affect the other consumer.

- Production Rust marker evaluation on the latest retained session processes 2,288 canonical frames.
  All 1,147 recorded SELECT observations resolve HYPER (old RGB predicate: 948 known, 199 unknown).
  This is recognition availability on an existing session, not a new-capture accuracy holdout.
  The two stationary failure spans recover all 40 prior unknowns in 112 observations.
- Additional retained legacy QOI evaluation covers 786 frames: all 75 recorded SELECT frames resolve
  (62 HYPER, 13 ANOTHER); 244 RESULT/PLAY/transition/mode controls accept no marker. Raw UNKNOWN
  frames include visible SELECT markers and are excluded from negative-screen accuracy claims.
  Ten manually labeled images cover all five SP slots and DP HYPER; all ten are accepted correctly.
  A separate 600-frame segment contains 166 SELECT frames across all five difficulties: known
  increases from 76 to 166, with no disagreement on previously known observations.
  DP other difficulties, other profiles and new-capture holdout remain unverified.

- The latest private session exposed the obsolete SELECT field-count validator: all 1,147
  successful SELECT field observations were rejected by diagnostic recording. Its canonical video
  and event stream are complete; the original partial session and private labeling draft remain
  unchanged. The new recorder persists the eight-field SELECT summary without semantic revalidation.

- Registered-model Rust production evaluation has 27 digest-bound manually labeled frames: 15 visible
  score panels and 12 transition/other-screen controls. SCORE, MISS and clear each accept 15/15;
  all 45 accepted fields are correct, with no control-frame acceptance. Includes
  recorded SP/DP SCORE/MISS, NO PLAY in SP/DP, FAILED, CLEAR, HARD CLEAR, EX HARD CLEAR and FULLCOMBO CLEAR.
  The parent accepted SCORE 11/15 and MISS 12/15; sampled digit 6 now passes the unchanged margin.
  Session groups share identical glyphs; this is not an unseen-glyph holdout. Frames and complete
  labels remain private.
- Current four-session production replay passes before and after interval/notification changes:
  22 accepted RESULTs, all labeled SELECT endpoints correct, 15,971 canonical frames. The complete
  RESULT and music-selection event streams are byte-equivalent as serialized values. Best snapshots
  remain 109 total (baseline 9/44/35/21, after 8/44/37/20); counts alone are not a correctness oracle.
- Resolver notifications decrease from 10,737 to 2,200. Adjacent notifications differing only in
  sequence/time or resolved difficulty streak decrease from 8,023 to zero. Baseline traces contain
  no same-chart adjacent snapshot restart candidates; retention correctness is also tested with
  explicit missing-evidence, conflict, mode/difficulty, suspension and delayed-job scenarios.
- Six manually reviewed frames label two short stationary intervals. Both baseline and updated
  replay have zero wrong chart/value associations and zero duplicate publications in those intervals.
  Three inspected scrolling frames have ambiguous central/list association and are excluded from
  stationary truth. These samples do not establish zero error across all scrolling transitions.
- SELECT lifecycle tests cover fresh two-observation recovery, no adoption while held, true revisit,
  contrary ambiguous candidates, existing unresolved mode conflict, stale work and result separation.
  Connecting snapshots preserve the last publication. Existing four-pane gate tests and held-state
  rendering pass at 120x40 and 80x25. Trace capacity/no-overwrite tests pass.
- `mise run check`, workspace/all-target Clippy and the complete default-parallel `mise run test`
  pass on the Event API v2/result-lifecycle snapshot. The suite includes 507 runtime library tests,
  20 score-store tests and 62 embedded-web overlay tests; focused score-store and overlay runs pass
  independently as well.
  Public API tests cover snapshot/live folding, provenance readiness, old queued-record exclusion,
  overflow with no subsequent event, idle reconnects/write-half-close, partial/slow clients, record
  limits, channel failure non-interference, and socket ownership cleanup. Raw diagnostic records and
  accepted RESULT payloads remain separate. Result-lifecycle coverage includes complete public-wire
  validation, provisional update/retraction/confirmation under one attempt identity, confirmed-only
  counts, v1 database migration, crash recovery under the database writer lease, snapshot reconnect,
  SQLite-driven overlay detail and explicit inactive/retracted lamps. The binding-mismatch fixture
  uses the existing isolated test supervisor. Independent final contract review has no remaining
  actionable findings.
- Trace provenance binds the running executable and both SELECT layouts; the three-file hash is
  explicitly a partial source fingerprint. Full production replay covers the final reducer/recognition
  behavior. Subsequent writer-provenance and test-fixture-only corrections pass focused tests,
  the complete suite and final review. Traces, six-frame interval labels and comparison reports are
  retained privately under `select-stability-evaluation-v1` in the scorepeek XDG data directory.

## Unverified and next execution boundary

- Complete 100 attach/detach cycles, two further 15-minute target runs and one 30-minute soak. Keep
  an exact Gamescope `out of buffers` count for those runs and establish whether scorepeek's private
  anonymous RSS reaches a stable bound under representative screen transitions. The confirmed
  1920x1080 target contract, one scorepeek-only exit that left Gamescope alive and one monitored
  15-minute run are encouraging but do not complete these gates; development-host vkcube lifecycle
  evidence likewise does not establish target INFINITAS support or repair a remaining Gamescope
  producer defect.

- Stream composition has all-three-skin development-host native PNG/layout/manifest inspection,
  alpha checks for overlapping apertures and frame-width content-geometry checks. Synthetic native
  title conversion/editing is separate from live Wayland keyboard focus, keymap and IME candidate
  delivery, which still need target validation. Composition assets respond through the OBS HTTP
  path. Development-host Codex Browser verification covers widget selection, double-click, move,
  four-corner resize, canvas/widget add-delete-undo, mixed-language title apply/cancel, frame/opacity/
  aspect controls, all-visible-canvases OFF, save/reload and discard/reopen. No normal configuration,
  live deployment or remote publication was changed.

- The score-store invalidation, RESULT readiness lamp, and parent-controller TUI isolation are
  development-host verified but not installed or exercised in a target-live game session.
- Target-live validation is still required for screen-driven surface visibility, configured
  opacity, forced cursor shapes/fallback, output hotplug while editing, cross-output interaction on
  integer/fractional multi-output layouts, Gamescope foreground behavior, readability, CPU/GPU/OBS
  lag and idle render cost. Actual OBS Browser Source composition and Interaction also remain
  unverified; the Codex Browser run exercised the same production HTTP/Wasm route but not OBS itself.
  The schema-v7 editor is not installed on the target and no autostart, push or release is included.
  Existing non-nested target evidence predates ADR 0144 and does not establish its new full-output
  stage lifecycle. The fresh nested-Scroll runs establish two-output protocol, composition, pointer
  input, empty bootstrap, creation, output reassignment and idle paint behavior, but not target
  GPU/compositor performance or output-hotplug reconciliation.
- OBS editor build compatibility is checked at connection and request boundaries. A mismatch
  discards the unsaved editor state, blocks edits and presents a reload button. The backend and
  WASM share a deterministic source/asset build identity. Socket-owned editor leases are released
  on disconnect; a readonly peer cannot discard another editor's draft. This does not retroactively
  add the reload notice to browser assets predating the compatibility guard. Development-host
  Codex Browser verification with two distinct builds confirms mismatch notice, removal of editing
  handles, reload, saved-state restoration, widget resize and successful save. Real WebSocket tests
  cover stale/missing identifiers, rejected saves and connection lease isolation; controller tests
  cover a former owner releasing after expiry and replacement by another editor.

- The release browser bundle disables DWARF debug symbols and completes without the prior
  wasm-opt DWARF failure. Shared editor hit regions sit above noninteractive rendered content,
  selected widget handles take precedence at canvas edges, and native pointer moves retain the
  actual pressed-button state. Manifest-defined canvas and widget properties now use a shared,
  typed editor card with human-readable labels, balanced choice segments, bounded numeric fields,
  units and dedicated toggle, color and text treatments in both native and OBS editors. Regression
  tests exercise body selection and edge-aligned resizing through Dioxus, plus delivered button state.
  Native render evidence remains distinct from live Wayland input and OBS composition.

- Validate layout v4 in a fresh target-live run with the installed binary.
  Retained-frame inspection does not recover unrecorded PLAY spans or backfill missing RESULTs.

- Confirm the new marker on target-live sessions.
- The previously observed diagnostic store root-lease test flake remains outside this change;
  the separately reproduced binding-mismatch test supervisor conflict is corrected.
- Additional capture coverage is needed for four-digit MISS, all remaining clear labels,
  other capture profiles, and target-live cost of the additional SELECT OCR jobs. The 27-frame
  best-value evaluation is a bounded sample, not a general zero-error claim. No explicit hidden-panel pattern
  is validated; `not_displayed` exists in the type but is not guessed from blank pixels.
- Current active private regression generation is
  `c4606091f2b2ca08686f4054a6cf080fc04f66a182c5177cbe9a9685b1ff4b20`: 4 sessions, 22 accepted
  attempts and 15,971 canonical frames. The three original entries are unchanged. The latest
  complete session now has its six manually reviewed RESULT/SELECT labels formally applied.
  The original three-session suite and the additional session were replayed separately with the
  same final runtime; all four sessions passed. Developer-host replay is not target-live verification.
- Public API developer-host verification and independent review are complete. The versioned wire
  contract is ready for consumer integration. Target-live API performance and capture gates remain
  unverified. Score persistence is developer-host verified only; target-live cost, release and push
  are not included.
