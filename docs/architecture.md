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
  CF --> SC["10 Hz screen classifier\nknown or unknown"]
  SC --> SCR["Semantic screen episode\nsuspend, drain, finalize"]
  SCR --> RE["Screen-specific field observers"]
  TM["PP-OCR text model"] --> RE
  NM["Fixed-cell HOG/MLP numeric model"] --> RE
  FC --> ER["Family-normalized joint hypotheses"]
  RE --> ER
  ER --> AR["Play-attempt resolver"]
  SCR --> AR
  AR --> RF["RESULT-close finalizer"]
  RF --> DE["Accepted domain event"]
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
The common receiver prefers 10/1 fps but accepts an unspecified producer rate,
returns superseded buffers within each PipeWire process event, and copies only
the newest mapped frame into application-owned memory. Recognition samples that
latest frame at its independent fixed 10 Hz cadence.

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

Screen-path-only evidence for `decide_transition` and `play` is stored in a separate,
digest-bound canonical-coordinate layout artifact. Adding those predicates does not change the
canonical field-layout digest bound by existing machine profiles.

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
PP-OCRv6 small native-dynamic observes result title, artist, clear type, difficulty, play type,
previous clear type, play options, and the music-select text fields. Fourteen fixed result numeric ROIs are instead
split only by canonical numeric-character layout v3. Each declared cell receives a field-family
hard/soft mask and a fixed 2,244-value HOG/soft-pixel feature. All cells are submitted together to
one registered `N x 2244 -> N x 11` MLP ONNX batch with classes `_0123456789`. Fixed-slot grammar
retains top-eight, all-blank, calibrated posterior, and margin evidence; it admits only leading
blank cells followed by contiguous digits and never discovers components from image content.
Display dashes are a separate fixed marker predicate rather than an MLP class. Offline marker
accuracy uses only operator-selected stable result frames; transition rows remain unsupervised
source evidence. The measured level variants cover one-digit ANOTHER/BEGINNER/HYPER and two-digit
ANOTHER/HYPER, while unmeasured variants fail closed.
Every due 100 ms tick evaluates the raw screen predicate independently of the bounded field worker.
The classifier produces only a known screen or typed unknown reason. `ScreenEpisodeResolver` owns
semantic continuity: raw unknown suspends the current known episode, the same known screen resumes
it, and only a different known screen, session boundary, or reversed chronology closes it. While
the field worker is occupied, crop submission alone is skipped as `field_observation_busy_skip`;
raw observation, semantic duration, and attempt path continue at screen cadence. Numeric model,
title geometry, candidates, and provisional decisions remain debug evidence.
Level remains advisory. A calibrated known level may add support only to candidates already
established by non-level song evidence and never rejects an otherwise supported chart. SP and DP
charts remain in the joint catalog distribution. Exact play type observed at least twice without
an opposite observation is required before chart acceptance and excludes opposite-type candidates;
difficulty and known notes alone cannot distinguish every sibling.
The catalog supplies the level of the accepted joint chart, while observed mismatch remains debug
evidence.
Each admitted field request carries its semantic episode ID and source sequence. Closing an episode
stops admission, drains all already-admitted jobs, applies their results to the closing episode,
and finalizes it before the next known screen changes attempt state. A different generation,
reversed chronology, or post-close submission is typed late evidence and cannot affect resolution.

The screen adapter converts full-catalog text metrics into song factors and retains typed chart
observations as independent play-type and correlated difficulty/notes/advisory-level factors. A common accumulator
keeps raw `u64` family sums across different source sequences. Summary projects those factors onto
the complete catalog hierarchy, then normalizes by the largest raw value in each family and, only
above 300, scales every candidate in that family by the same ratio. This caps repeated evidence
while preserving candidate margins. Empty or unknown fields add zero and never erase earlier
support. RESULT keeps one accumulator for its screen episode. Resolver authority receives the full
typed candidate hierarchy. Run-event and observation sinks construct only bounded top-candidate
JSON projections instead of serializing the full authority graph.
MUSIC SELECT keeps incumbent and successor selection epochs. Intersecting evidence accumulates in
the incumbent; disjoint evidence accumulates in the successor and replaces it at the calibrated
change margin. Difficulty observed without song evidence is retained until the next credible song
observation chooses its epoch. After screen close stops admission, already-admitted fields drain;
only semantic finalization hands the latest unfinished successor or incumbent to the attempt.
Empty and catalog-common observations cannot move an epoch.

All active-list titles use one foreground-aware extractor. It masks grayscale values above 80,
takes the complete foreground bounding box with four horizontal pixels of margin and the original
ROI height, then runs the registered dynamic PP-OCR runtime. Wide OCR remains raw diagnostic
evidence; foreground OCR is lexical authority. Empty and whitespace-normalized values are absent.
Foreground Unicode scalar count and width contribute structural support to the same select-title
family, using the maximum of lexical and structural support for the crop. Raw `X` remains lexical evidence for catalog `X`; it is never aliased
to `〆`.

MUSIC SELECT submits only central title, artist, and active-list title to PP-OCR. Difficulty comes
from five fixed canonical `PLAYER 01` marker slots in integrated-context layout v4. The shared RGB
panel/fill/glyph predicate requires exactly one winner above both a minimum and margin; all other
states remain typed unknown. Difficulty support narrows charts only under an already text-supported
song and cannot generate song identity.

The attempt resolver owns select and result evidence snapshots rather than an armed song and a
separate result song. It combines them once on the same `(song, play type, difficulty)` catalog
hierarchy. SP and DP remain sibling candidates until conflict-free two-observation play-type
evidence distinguishes them; chart acceptance waits for that evidence and then rejects the
opposite type. Difficulty and notes may be identical across those siblings. A
select screen preserves linkage without accepted identity, allowing sufficient RESULT evidence to
finish an observed select/play/result path. Returning through MUSIC SELECT starts a fresh context;
only direct RESULT-to-PLAY inherits its parent selection once without re-adding frame support.

RESULT identity, clear type, and fixed-cell numeric performance remain provisional while the screen
is displayed. After semantic RESULT close and admitted-field drain, finalization validates path,
joint identity, clear type, score invariants, and required numeric tuple exactly once. It records a
confirmed `play_attempt_changed` before the attempt's sole v2 domain event. Unresolved or conflicting
final state completes the attempt with a typed reason and emits no domain result. OCR produces raw
and typed observations; it never owns catalog acceptance or stability.

The active-suite video-replay path can produce an
immutable operator-review draft. It reproduces 10 Hz packet-order sampling and measures the right
list, active row, and central title independently through the production normalizer. Full-frame
animation is deliberately excluded, 500 ms transition context is retained, and all spans remain
unknown until operator review. Digest-bound operator intervals are expanded to exact adjacent
sample pairs. Pairs whose two retained predicates are music-select require an operator decision,
but an operator can exclude a visibly different screen as typed context rather than inventing a
motion state. Predicate-derived and operator-derived context remain distinct unknown states.
When selection identity changes while the right list also moves, operator review records
`selection_change`; motion with the same active identity is `scrolling`, and only unchanged
identity plus unchanged list placement is `stationary`. Non-list animation does not affect this
classification.
Partial reviewed sets expose completeness and cannot become temporal-policy truth. The corrected complete
bound set contains 713 stationary, 83 scrolling, 30 selection-change, 12 operator-context, and 133
predicate-context adjacent pairs. It supplies motion evidence for temporal-policy evaluation but
does not itself establish song correctness.
The historical equal-ID dwell evaluator joins that complete truth with the bound observation stream and exact
catalog generation, replays the production frame-local resolver, and compares equal accepted-song
durations of 100--500 ms. After correcting two prematurely authored identity boundaries, every
tested duration resets all selection changes with prior stability. Stable output during
same-identity list scrolling is recorded as neutral nonstationary activity, not a false song
decision. Longer dwell still reduces stationary-run coverage, and motion truth alone cannot select
a runtime dwell by itself. A separate complete correct-song set labels all 27 maximal stationary runs as 18
songs and nine category/filter selections. Against those 740 observations the frame resolver has
no wrong accept or accepted-ID transition; the leading 200 ms candidate preserves 16/18 song-run
coverage and zero wrong/non-song stability, but the rejected clear-on-unknown reducer increases
unknown observations from 11 to 35. ADR 0067 supersedes that reducer with the production
hold-and-replace state machine and evaluates the 100/200/300/500 ms dwell by 100/200/300 ms grace
matrix. Combined with the corrected motion truth, it selects the 200/200 ms provisional runtime
presentation described above. Operator truth remains evaluation-only and never enters runtime;
raw central-title text variation remains separate from song-ID state. Neither the offline evidence
surface nor `temporal_music_select_changed` grants accepted event authority.

Independent canonical predicates classify decision and gameplay without routing OCR. Missing
decision transition remains acceptable, while PLAY or RESULT without select/retry linkage and a
result without observed play remain typed unlinked rejections.

Every inspected source sequence has one diagnostic field status. Busy, non-applicable, and rejected
submissions are recorded after the synchronous screen/output path; completed and late outputs are
recorded after the field worker and attempt/output path return. Recognition observation v15 carries
title views and geometry, the typed MUSIC SELECT marker, semantic episode binding, song factors,
typed chart observations, numeric decisions, and completed/late status. Typed v4 resolver
transition events retain incumbent/successor/result/joint top, distinct other-song and sibling-chart
runners, state, and raw/normalized family contributions.
Frame timing records actual screen resolver, attempt resolver, and synchronous output durations on
the originating source sequence; async queue wait is excluded. Optional stage timings distinguish
an unexecuted stage from a measured zero and never affect recognition or event semantics.

The offline corpus tool can replay this exact production reducer over ordered, operator-reviewed
result intervals and compare bounded observation-count/gap policies. Its versioned JSON report
keeps raw per-frame correctness, final temporal outcomes, transition counts, and stabilization
latency separate. The report is descriptive evidence bound to one active private-suite generation;
it neither calibrates thresholds nor grants event or release-accuracy authority. See
[the temporal evaluation contract](temporal-evaluation.md).

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

Screen-local evidence remains provisional and cannot grant confirmed or persistence authority.
Once joint identity and the two-observation numeric tuple resolve for an active RESULT attempt, the
resolver may publish a typed provisional lifecycle value. Semantic episode close and attempt
finalization remain the only confirmed promotion boundary; completed attempts, retry parentage,
and event deduplication therefore belong to the resolver rather than OCR or output presentation.

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
outside the capture handoff and belong to a separately owned application execution boundary.

ADR 0030 supplies that application execution boundary.
One loader receives the exact immutable run binding once before capture work, then its observer is
owned by a single bounded worker. Only opaque crops from the same run ID and full binding enter the
capacity-two non-blocking queue. Worker results receive provenance outside the observer output;
the same capacity also bounds accepted but unconsumed results after queue removal. Queue full,
outstanding-result limit, worker loss, abandoned results, and bounded finish timeout remain typed.
The single-production-worker token is retained through observer teardown. Model/catalog loading,
field schemas, catalog decisions, and event acceptance remain separate layers.

ADR 0031 supplies the production resource loader for that boundary. It requires the active catalog,
registered PP-OCRv6-small text model, and an explicitly installed active private numeric model to
match their immutable manifests before the production worker starts. It retains one text ONNX
session and one specialist numeric ONNX session. Missing or changed numeric bytes fail closed; the
text recognizer is not a numeric fallback. The read-only resource gate transfers the loaded
resources into the production field worker and requires bounded teardown without crop submission.
It proves resource admission and worker ownership only; it is not live recognition or performance
evidence.

ADR 0032, as narrowed by ADR 0036 and superseded for implemented fields by ADR 0092, supplies the
production screen-field observer and exact complete output shapes. Result output contains five
general-text observations and fourteen specialist numeric states; music-select contains its four
general-text observations. A text or numeric-batch failure returns a typed whole-screen error
instead of a partial value. The existing field-count operation records the screen, fixed field
count, and an optional typed failed-field ID. ADR 0037 requires the application-owned recognition artifact to
retain bounded exact OCR strings, a run-scoped exact catalog display/comparison string table,
candidate string references, and metrics as stages are added; this evidence still has no
song-decision, accepted-field, suppression, or event authority by itself. Pixels remain in the
separate bounded image store.

ADR 0036 adds source adapters on the other side of the shared bound-canonical owner. The Gamescope
adapter acquires and normalizes an admitted frame; the recording adapter reads a profile-bound
canonical extraction derived from a corpus recording. Both then enter the same recognition session,
crop router, registered worker, and candidate domain. A create-only recording simulation profile
binds recording/extraction/layout/resource provenance, ordered result windows, exact expected
`CLEAR TYPE` text, source pacing, and diagnostic sampling. Result presence uses the fixed result
header and panel boundaries rather than the variable result background. This replay gate grants no
accepted field, song, event, live-profile support, or performance authority.

ADR 0038 adds result-screen song authority after that shared candidate path. The pure resolver uses
the unique minimum title edit-distance candidate, requires title distance at most one, normalized
title similarity at least `6/7`, and selected-candidate artist similarity at least `2/5`. ADR 0093
requires margin one for a unique exact title and retains margin two for fuzzy title evidence.
Artist evidence corroborates the title-selected song and is not added to title distance. Every
rejected condition is a typed unknown. Recording profile v2 binds an exact
expected song ID per episode and requires two exact song decisions plus two exact `CLEAR TYPE`
observations. The create-only local artifact retains the exact OCR, catalog strings, complete
candidate metrics, decision/reason, and expected values; it does not duplicate pixels.

ADR 0039 connects that same value-bearing serializer to live Gamescope results without putting
filesystem I/O on the capture loop. A capacity-two worker receives completed registered
observations non-blockingly and writes the create-only catalog, observation stream, and final
manifest. Observation schema v2 tags recording PTS separately from live bound monotonic start/end
times. Queue full, worker loss, write failure, and finish timeout remain typed artifact degradation;
they cannot replace recognition. The live value-evidence command passes only when every completed
observation was enqueued, at least one completed result resolution exists, and the manifest
completed. Its top-level status and exit agree on artifact failure. A process-wide supervisor
rejects a second writer until a timed-out predecessor actually exits; that predecessor may finish
an already-started publication, but its run remains failed. The older counts gate retains its v1
schema, while the new value-evidence gate has a distinct v1 schema. Compact command JSON contains
counts, status, and digest, while the local artifact retains the exact OCR, song IDs, catalog
strings, candidate metrics, and resolver decisions.

ADR 0033 joins that observer and the diagnostic-backed recognition session under one application
owner created from the same immutable descriptor. Resource loading completes before the recognition
run opens. Each frame reports screen inspection, non-blocking field submission, and diagnostic
queueing separately; unknown screens do not submit. One pending result can be consumed once, and a
private owner token prevents another run from consuming it. Completed and disconnected channels are
terminal after their first result. The owner retains an exact capacity-two pending-sequence ledger.
Finish closes the field worker first, records each remaining sequence as abandoned and lifecycle
failure as unbound while the diagnostic run is still open, then finalizes that run. This owner is
execution and provenance plumbing only; it does not resolve catalogs or accept fields, songs, or
events.

ADR 0034 adds a pure immutable candidate domain derived from every song in the active catalog.
For each result title and artist, or each music-select central title, artist, and active-list title,
it retains every song with independent minimum edit distance and exact integer maximum normalized
similarity. Raw, exact comparison-key, and collision-safe folded forms share the registered
comparison-key contract; folded observations compare only with domain-unique folded candidate
forms. A search-term-only song makes domain construction fail with its typed ID rather than being
dropped. The music-select title presentations remain separate consistency evidence, not independent
votes. This boundary does not rank, truncate, intersect, stabilize, accept, suppress, mutate
context, record diagnostics, or emit events.

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
oldest inactive run, regardless of priority, as exact new publications require space;
it proves the publication can fit after all eligible reclamation before the
first capacity deletion, and rolls back uncommitted byte reservations.
An active run is never removed. Priority keeps its longer expiry period but is not a capacity pin;
operators export evidence that must outlive the bounded store. Completed-run age uses
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

The ordinary foreground runtime exposes provisional recognition observations at
`$XDG_RUNTIME_DIR/scorepeek/observations-v8.sock`. A connection begins with a bounded v8
current-state snapshot and then receives sequenced `scorepeek-run-event-v8` NDJSON. This local
observation surface may include raw OCR, foreground title geometry, joint candidates, and resolver
metrics. `raw_screen_observed` is separate from semantic episode started, suspended, resumed,
closing, and finalized transitions; `play_attempt_changed` contains the evidence-linked path and
typed final relation without altering raw recognition;
`next_channel_sequence` marks the first event not represented by the snapshot so a client can
discard an already-represented live record and detect later gaps. It is intentionally separate from
accepted domain events. TTY stdout renders the same typed run state as a TUI, while non-TTY stdout
reports only human-readable state changes.

`result_provisional_changed` carries the same `scorepeek-result-detected-v2` payload as the
confirmed `result_detected` event. Its episode-local revision orders resolved, replacement, and
withdrawn states. The outer event kind, never the nested payload contract, distinguishes UI-only
provisional state from confirmed score/history authority. Both are retained in diagnostic
run-event artifacts; only `result_detected` is a confirmed result.

Screen-local and attempt resolvers accumulate title and artist song factors independently from
difficulty, notes, and advisory-level chart factors. Chart factors are retained across observations
and are projected only onto songs established by text evidence. Summary selects the best chart per
song, reports a distinct best other song and best sibling chart, and requires both margins for joint
acceptance. Foreground lexical and geometry title features share one family and contribute their
maximum rather than two votes.

RESULT play options use the canonical whole-panel ROI `(30, 318, 530, 50)`. PP-OCR reads the whole
label while a fixed orange-marker predicate independently distinguishes a positively absent label
from inconclusive blank OCR. The parser compares the normalized observation against the complete
finite language of ordered, distinct RANDOM, R-RANDOM, S-RANDOM, MIRROR, A-SCR, and LEGACY displays
and accepts only a unique minimum at edit distance zero or one. Two matching typed observations in
one semantic RESULT episode produce a known ordered list; conflict or incomplete evidence remains
optional unknown and cannot suppress an otherwise accepted result.

MUSIC SELECT difficulty is the exception to historical factor accumulation. A typed-known marker
is current selector state: a different known value replaces it on the first observation, equal
known observations increase only its bounded chart support, and unknown observations retain the
last known state. Difficulty-only frames update the active successor or incumbent without adding
song evidence; before any credible song they replace a single pending state. Snapshot and retry
composition select the newer source sequence instead of adding difficulty history.

The TUI has one vertical layout: four rows for Watcher, nine for Latest result, and the remaining
rows for Resolver. Latest result prefers an active `PROVISIONAL` v2 payload, restores the newest
`CONFIRMED` result after withdrawal, and adds only confirmed events to count/history. Resolver
formats a typed tree containing raw and semantic screens, field age, incumbent/successor or result
evidence, foreground title geometry, hierarchical runners, family contribution, attempt hierarchy,
and every promotion gate. Raw marker and resolver-current difficulty are separate values, including
the consecutive-known count. Green, cyan, yellow, red, dark gray, and white encode typed semantic
state consistently while the same labels and gate symbols preserve meaning without color. It
shows integer-second monotonic durations from the private 10 Hz tick, but that redraw creates no run
event, socket record, plain-output line, or domain event. Raw OCR is limited to the current screen's
important fields; full candidate sets, logits, and frame timing remain in artifacts. Terminals below 80 by
25 are allowed to clip without a second layout.

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
