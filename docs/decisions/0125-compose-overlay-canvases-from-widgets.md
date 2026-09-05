# ADR 0125: Compose overlay canvases from independently placed widgets

- Status: Accepted
- Date: 2026-09-05
- Supersedes: ADR 0122's fixed three-panel composition and input-transparent Wayland surface; ADR 0123's startup-only skin and compact/sidebar layout selection.

## Decision

The overlay configuration is a strict, versioned TOML document. `scorepeek run` enables the native
and OBS backends independently with `--overlay-wayland` and `--overlay-obs`; `--overlay-config PATH`
selects the document and otherwise uses `$XDG_CONFIG_HOME/scorepeek/overlay.toml`. A missing document
is atomically created with one enabled 560x1040 canvas for each backend and the five master widgets.
Global/schema errors reject the document. An invalid canvas is isolated, while each backend must
retain at least one valid and one enabled canvas.

A canvas belongs permanently to one backend. Wayland creates one layer surface per enabled native
canvas inside one child. OBS exposes each enabled browser canvas at the stable `/canvas/<id>` URL;
the browser-source viewport owns its outer dimensions. Canvas and widget coordinates are logical
top-left pixels. Widgets have persistent IDs, kind, 4px-aligned position and size, z-order and typed
settings. The initial kinds are status, selection, score, history list and history graph. Shrinking
a widget reduces internal spacing before clipping; it never scales its typography.

An explicit native `output` remains authoritative while it exists. When it is absent or no longer
connected, Wayland chooses the first named connected output in stable name order so a multi-output
first run always exposes the editor, then persists the resolved name through the parent writer.
Choosing another output in the editor persists its name and recreates the surface there.

Right-clicking anywhere enters the editor. The only editor exit is DONE. Wayland remains mouse
interactive: normal left-drag moves its canvas, while edit mode moves/resizes widgets or the canvas.
OBS uses the same URL and DOM in Browser Source Interaction; normal drag remains available to OBS
Transform rather than moving persisted canvas geometry. Both editors expose the three skins,
widget addition/removal, history list count (5/10/20/50), graph range (1/3/6/12 months), and 4px plus
edge/center snapping. OBS also manages its canvas list. Configuration writes go through the parent;
per-canvas 15-second leases, canvas revisions and backend-list revisions reject concurrent stale
edits. The parent validates and atomically replaces TOML, restoring memory state on failure.

The five widgets project meaning rather than recognition internals:

- status shows the scorepeek wordmark plus labeled SYSTEM and RESULT lamps. SYSTEM is active only
  when the active session's required catalog/model and enabled score/recording paths are ready.
  RESULT shows processing, persisted or failed for the current result ingest.
- selection shows an unlabeled vertical recorded lamp, title/artist, and a separate compact rail for
  SP/DP, difficulty, level and notes. Recorded means committed DB knowledge exists, including an
  explicit no-record SELECT observation; it never displays score values.
- score shows the DB-derived integrated BEST and a representative RESULT DETAIL without repeating
  the song. The representative result is highest EX score, then known/lower miss, then latest
  receipt time. RESULT-only judgments, timing, combo break and play options remain visible.
- history list shows local notification time, EX score, DJ LEVEL, miss and clear.
- history graph fixes its time domain to the configured range and plots exact receipt timestamps.
  EX score is expressed only against labeled DJ LEVEL thresholds. Miss rate uses a fixed 0–100%
  axis; values above 100% are clipped and unknown values leave gaps. The query retains the newest
  4096 plays within the selected maximum range, then restores timestamp order before drawing.

All score, recorded and history values are read back from committed SQLite state. SELECT changes,
SELECT-best publications, successful RESULT persistence, and a five-second recovery interval trigger
the read-only query. UI state does not treat the latest public RESULT as the score widget's authority.

The normal canvas is transparent outside widgets. Each skin uses the approved generated design
master as visual authority. Shared semantic DOM may contain decorative boxes; skin CSS may use
embedded PNGs as backgrounds or masks. Runtime values remain text/SVG rather than baked into images.
CYAN SYSTEM, RESULT AURORA and DJ BLACKBOX share the same state logic. Oxanium is embedded for Latin
and numeric text under OFL 1.1; Japanese continues through system-font fallback. Animation is limited
to a finite result-state emphasis and settles to an idle render.

## Consequences

Canvas configuration replaces the old layout and appearance CLI switches. Existing overlay TOML is
schema-versioned but this first schema has no compatibility promise. OBS URLs are suitable as stable
Browser Source identities even though canvas contents and skin change. Native output changes and
new/deleted native surfaces require the child surface owner to reconcile compositor resources; those
operations remain observable and fail independently of recognition and score persistence.

Development checks cover strict parsing, invalid-canvas isolation, atomic writes, lease/revision
conflicts, native DOM layout and idle animation, WASM compilation and browser asset embedding.
Compositor output bounds, integer/fractional rendering, game readability, OBS interaction and target
idle GPU behavior remain explicit target-live gates.
