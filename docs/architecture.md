# Architecture

This document is a compact map of the intended system. The accepted delivery
sequence and release gates are authoritative in [the implementation plan](plan.ja.md).

## Data flow

```mermaid
flowchart LR
  G["Gamescope provider\nfirst spike"] --> SA["PipeWire source lease"]
  P["Portal provider\ndeferred"] --> SA
  CP["Registered custom provider\ndeferred"] --> SA
  SA --> PR["Common PipeWire receiver"]
  PR --> OF["ObservedFrame\nopaque capture profile"]
  OF --> DN["Versioned domain normalizer"]
  DN --> CF["CanonicalFrame\nRGB8 1920x1080"]
  CL["Canonical layout\nshared game coordinates"] --> CF
  T["Tachi"] --> AD["Source adapters"]
  TX["Textage"] --> AD
  D["INFINITAS roster signal"] --> AD
  AD --> FC["Federated catalog snapshot"]
  CF --> RE["Rust field recognizers"]
  CF --> OP["Versioned OCR preprocessor"]
  OM["Pinned ONNX sequence model"] --> CTC["Catalog-constrained CTC decoder"]
  OP --> OM
  RE --> CTC
  FC --> CTC
  CTC --> SR["Screen-context song resolver"]
  RE --> SR
  SR --> SC["Minimal stable-selection song context"]
  SC --> DE["Deterministic domain event"]
  DE --> ENV["Daemon transport envelope"]
  ENV --> API["Unix socket NDJSON v1"]
  API --> UI["Future UI and consumers"]
```

## Stable boundaries

### Frame source

PipeWire source acquisition is separate from frame reception. A selected
provider acquires a lifetime-bound source lease containing its remote, exact
node or deterministic selector, capture profile, and provider-owned lifetime
guard. The common receiver owns stream negotiation, bounded latest-frame
handling, sequence/timing, and source-loss notification. Gamescope against the
default PipeWire remote is the first provider. Portal later supplies a
session-scoped remote FD and node ID through the same boundary; registered
custom providers may follow. No provider silently falls back to another.

Every backend produces an owned `ObservedFrame` containing its exact input
contract, capture generation, sequence number, monotonic timing, and immutable
opaque capture profile identifier. A versioned domain normalizer maps that
profile to an owned, contiguous RGB8 `CanonicalFrame` at exactly 1920x1080.
The canonical representation is the specified logical game canvas; it has no
required native, Portal, Gamescope, or OBS pixel reference. A capture profile
owns only observed input. Its normalizer artifact maps that profile to one
canonical frame contract. The canonical frame contract owns the shared game
layout; replay evidence references its immutable layout digest separately.

The normalizer treats its capture pipeline as one domain and does not model
Wine, Vulkan, Gamescope, compositor, PipeWire, or other layers separately.
Deterministic geometry, color, and filtering are preferred. A learned residual
adapter must be justified by measured recognition evidence, remain bounded and
deterministic, and never act as a generative text restorer.

Portal, Gamescope direct PipeWire, and any later eligible route remain peer
profiles selected by independent semantic, lifecycle, and target performance
gates. Sharing a PipeWire receiver does not make their observed pixels or
lifecycles equivalent. Each candidate has a distinct capture profile and
normalizer but does not own a layout. A running session never silently switches
providers or profiles, and reacquisition starts a new capture generation.

### Layout

One versioned canonical layout contains scorepeek-owned ROIs, presence
predicates, and alignment tolerances in logical game coordinates. Every
supported capture profile must normalize to that layout. The layout changes
only with the game UI geometry, canonical frame contract, or field contract;
capture-route changes create a new profile or normalizer instead. An upstream
implementation may inform where to investigate, but its code, coordinates,
resources, and derived data do not enter the layout or its generation process.
The initial layout may be measured from one exact profile only after its
versioned normalizer has produced canonical frames. That profile is calibration
evidence, not a pixel reference. Later peer profiles calibrate their own
normalizers to the same layout rather than creating route-local coordinates.

### Catalog federation

Source adapters turn immutable Tachi, Textage, and INFINITAS-roster snapshots
into typed observations with source revision, lineage, content digest, parser
version, field authority, and scope. They never execute downloaded JavaScript.

Federation uses exact source bindings and exact, multi-field evidence. It does
not use fuzzy identity merging, weighted majority, or source recency to settle
cross-source conflicts. Ambiguous records are quarantined independently while
safe additions extend the previous catalog. A content-addressed SQLite snapshot
is activated by atomically replacing a small manifest; source failure never
shrinks the last-known-good catalog. Scheduled and manual sync share one
per-host exclusive writer lock. Activation verifies that its base digest is
still current, fsyncs staged files and directories, renames the snapshot, and
fsyncs the content-store destination parent. Only then does it fsync and
atomically replace the active manifest and fsync the manifest parent before
releasing the lock.

Scheduling is an outer trigger for `scorepeek catalog sync`; it does not encode
how a catalog is acquired. The current implementation builds from validated
source observations on each host. If a later ADR and source permissions allow a
GitHub-managed catalog, the same command can expose a user-selected self-build
or immutable provided-catalog mode, while GitHub scheduling runs the self-build
orchestration before publication. Both modes must retain provenance, bounded
content-addressed storage, semantic validation, and last-known-good activation.

The daemon does not synchronize during a game session. It opens one immutable
catalog digest at startup.

### Recognition

The engine exposes pure frame inspection, catalog-constrained title decoding,
and a separate stateful session boundary:

```text
RecognitionEngine.inspect(frame) -> RecognitionSnapshot
TextObserver.observe(roi) -> TextObservation
ScreenSongResolver.resolve(observations, catalog, context) -> AcceptedSong | Rejected
RecognitionSession.process(snapshot) -> DomainEvent[]
```

The canonical frame preserves RGB information shared by all field recognizers.
Recognition constructors require the expected canonical-extraction digest and
validate the complete typed normalizer artifact, canonical extraction manifest,
frame bytes, and their digest bindings; a bare observed RGB frame is not
accepted even when its dimensions happen to be 1920x1080.
After layout-bound ROI extraction, OCR-specific grayscale, contrast, resize,
padding, and tensor normalization belong to a versioned OCR preprocessor bound
to the model. Training and Rust inference use the same preprocessing contract.

Fields are represented as `known`, `unknown(reason)`, or `not_applicable`.
PP-OCRv6 small native-dynamic is the selected v1 text observer for title and
artist; each field keeps its own ROI and preprocessing contract. OCR output is
an observation rather than an authoritative value. Full-catalog resolution
requires temporal agreement and screen-specific context. Result uses title,
artist, play mode, difficulty, level, and notes. Music select uses central
title, artist, play mode, selected difficulty and level, and the active
right-list title. The two title presentations are not counted as independent
metadata votes, and readable conflict rejects. Version participates only when it is
independently recognized. Detected events require every mandatory field to be
known and cross-field validation to succeed. Rejection is preferable to a guess.

Python is an offline training/export dependency only. The game-session runtime
uses a pinned ONNX model in Rust and has no model or catalog network fallback.
Real captures, training labels, source snapshots, generated catalogs, and model
artifacts remain outside the repository.

Human-labelled captures from any supported profile may extend an immutable
corpus generation and produce a new normalizer or shared recognizer bundle.
Promotion requires frozen in-profile and cross-profile replay without a
regression; runtime self-labelling, online training, and automatic threshold
relaxation are prohibited.

The session output is deterministic for recorded inputs. UUIDv7 IDs and wall
clock delivery timestamps belong to a daemon-owned transport envelope, not the
recognition result compared by replay tests.

A screen-local episode stabilizes its own fields. The stateful recognition
boundary retains only the last stable music-selection candidate set. Result
candidates may intersect with it to improve song uniqueness, but context never
substitutes for result detection, savability, or result-only fields. Neutral or
unrecognized scenes preserve context; a new stable selection replaces it; a
confident title/session end, coverage gap, or recognition-binding change clears
it. Result processing does not consume it because result-to-gameplay replay can
occur without another selection. Mode, attempts, retry count, and full-session
composition remain outside recognition-core ownership.

Development uses a small number of scenario recordings, including a retained
ordinary full-session recording. The complete game flow remains validation
material even though it is not a runtime state machine. Bounded local live
diagnostics are sampled independently of recognition success and preserve
replayable canonical evidence, sequence/timing, transitions, immutable
bindings, decisions/outcomes, and completeness. This makes missed detections
observable after the fact. The manifest measures leading, adjacent, and
trailing unobserved intervals over an explicit run boundary; gaps and drops
downgrade completeness and cannot prove result absence. Denominator eligibility
remains false until a separate immutable multi-recording calibration artifact
exists and binds an accepted minimum result dwell. Diagnostic failure cannot affect recognition or
event delivery, and remote export is disabled without opt-in.

ADR 0025 fixes one diagnostic run to one immutable capture-generation binding.
The provisional sampler policy is 1 Hz with an 8 GiB aggregate local budget;
it is not result-denominator evidence until minimum result dwell is calibrated
and the measured run gap satisfies that bound. Lossless QOI canonical frames,
semantically consistent operation-scoped typed fact documents, a digest-bound
start document, bounded reason-bearing missing ranges with explicit truncation,
and a manifest-last completion record with exact storage byte accounting
form the private replay surface. The application owns queueing, retention,
health, deletion, and export; none belongs to `SongContext`.

ADR 0026 places the synchronous writer behind an application-owned,
single-producer bounded worker. Live offers transfer owned canonical frames with
the 1 Hz cadence gate applied before non-blocking `try_send`; a dense capture
stream therefore does not fill the queue with frames the writer would skip.
The producer, rather than the sampled writer, accounts for true capture sequence
gaps. Queue full and worker loss degrade only the diagnostic run. Caller waiting
for completion is bounded to five seconds, and a single-worker supervisor bounds
a residual blocked thread. Timeout is not terminal: a worker already in
filesystem publication may still produce the authoritative manifest later. The
strict offline replay control
uses the same queue and writer, digest-binds its request and canonical
extraction, and may wait only within that bound so offline producer speed does
not simulate a live drop. Replay timing is the extraction's exact source PTS,
not a caller-authored timeline. This replay is recognition-free and does not
infer a session timeline.

The application live handoff now accepts only fixed-size canonical RGB8 frames
with one capture-generation/profile/normalizer binding. Immutable pixels use
shared ownership so the producer can offer diagnostic evidence before recognition
without a second RGB allocation; the worker still owns QOI and filesystem work.
The offer is non-blocking. An application-owned live recognition session validates
the complete immutable diagnostic descriptor and embedded layout, offers each
matching frame to diagnostics before inspection, and rejects binding drift before
recognition. Its stable binding identity covers generation, capture profile,
normalizer, layout, catalog, model, and runtime inputs. Explicit rollover records
the next identity in the old run, finishes that run, and only then creates a new
session; this resource lifetime does not infer a game session. Diagnostic opt-out,
queue loss, worker loss, and persistence failure preserve the caller's recognition
observation. This is the producer/worker and screen-predicate integration boundary,
not an accepted field observer, supported profile, or target-host performance claim.

Supported live screen observations route through the same filesystem-free screen-local crop
function used by offline artifact export. The result branch structurally requires its measured
title, artist, difficulty, level, notes, and current-score crops. The music-select branch requires
its measured central-title, artist, selected-chart, and active-list-title crops. Unmeasured fields
are not represented by empty optionals, and the unknown branch cannot construct field crops. Each
live branch is an opaque owner retaining a borrow of the admitted frame; only the session constructs
it, and callers can obtain only a borrowed screen-specific crop view. These values have no OCR,
song-decision, accepted-field, or event authority. Model bundle I/O and inference therefore remain
outside the capture handoff and require a separately owned application execution boundary before
live field observation is implemented.

The first read-only application controls inspect an existing diagnostic root
through a shared strict inventory. `status` exposes fixed retention policy and
the current exclusive-writer state plus bounded aggregate byte/completeness
counts; `list` exposes only opaque run
identity, start/manifest digests, terminal state, priority, and managed bytes.
A valid start document without a completion manifest remains observable as
priority partial evidence. Inspection fails closed on unmanaged entries,
symlinks, typed manifest or exact file-set drift, per-run or aggregate capacity
overflow, and concurrent mutation of any directory in the store snapshot.

The application holds exclusive locks on the store-root directory inode and a
canonical-root-path-derived zero-byte ownership anchor in its stable parent for a whole
diagnostic run; the durable zero-byte root marker is only an inventory sentinel
and is not the lease identity. This is an advisory cooperative-writer contract,
not a defense against deliberate same-UID replacement of both root and anchor.
Under that lease, retention removes expired runs and then the
oldest non-priority normal runs only as exact new publications require space;
it proves the publication can fit after all eligible reclamation before the
first capacity deletion, and rolls back uncommitted byte reservations.
an active or unexpired priority run is never removed. Completed-run age uses
manifest publication time, while a crash-left partial uses its directory-entry
publication time. Deletion is rename-first and recoverable through a durable
scorepeek-owned marker that binds the run ID and exact pre-delete file
inventory. Marker publication itself uses a fixed recoverable staging state.
Recovery accepts only a remaining subset of that inventory, and
orders payload/marker unlink with directory fsyncs before the final root fsync,
so a crash after partial cleanup resumes on the next writer. Digest-confirmed
freeze publishes an in-run priority marker and preserves non-regular or symlinked
reserved staging as invalid state; explicit delete reuses the same
rename-first deletion state machine. Complete-run local export rehashes every
manifest-bound artifact, copies into an absolute create-only directory, and
publishes `export.json` as the last fallible commit point with atomic create-only publication. It resolves the
existing destination parent and canonical store root before creation, so aliases or
intermediate symlinks cannot route an export back into the managed store. Export failure leaves an observable incomplete
destination rather than overwriting or guessing cleanup ownership.
Observed pathname identity drift fails closed; adversarial replacement after a
final identity check is outside the operator-trusted private-artifact boundary.

### Event API

The first public interface is a same-user Unix socket at
`$XDG_RUNTIME_DIR/scorepeek/v1.sock`. It streams versioned NDJSON accepted
domain events, never pixels, OCR candidate text, source snapshots, or stored
history. Future UI code must consume this API and must not import recognizer,
catalog-adapter, or capture internals.

## Ownership

| Concern | Owner |
| --- | --- |
| External source bytes | Each Bazzite host's private cache |
| Catalog identity, federation, and activation | scorepeek catalog core |
| Capture normalization and profile-to-contract mapping | Versioned normalizer artifacts |
| Canonical game coordinates and shared layout | Versioned canonical frame contract |
| OCR preprocessing, model artifacts, and thresholds | Versioned scorepeek model bundles |
| OCR training corpus and model artifacts | External private store |
| Recognition and temporal semantics | scorepeek Rust core |
| Bounded replayable live diagnostics | scorepeek application, outside the public event stream |
| Public event compatibility | scorepeek schema version |
| Future UI and persistence | Later scorepeek applications |
