# Private corpus contract

## Frame-first capture regression (current)

ADR 0056 replaces the former recording-root capture dataset with this flow:

```text
live run     -> 10 Hz recognition -> diagnostic v3 -> review -> corpus
video replay -> 10 Hz recognition -> diagnostic v3 -> review -> corpus
```

The only importable capture-regression input is a successfully verified
`scorepeek-private-diagnostic-session-v3`. Facts, recognition observations, and watcher/session
events are session NDJSON streams. Ordered canonical evidence is RGB8 1920x1080 QOI and is
content-deduplicated; video and observed-frame evidence are optional. Recognition output is a
review aid, never ground truth. Video replay evidence stops at 1,024 frame references or 1 GiB of
unique QOI bytes without stopping recognition or the fact/observation streams. Each NDJSON record
is bounded to 1 MiB and each session stream to 250,000 records; reaching the stream bound is
recorded as degradation. Corpus replay runs the production predicate over every retained canonical
frame, while OCR/catalog/result-field expectations apply only to operator-labeled stable frames.
Value-bearing records keep exact OCR and resolver output but bind a candidate count to the separate
exact catalog object instead of duplicating every recomputable per-song metric at every tick. The
complete observation stream is bounded to 512 MiB.

New regression labels use `scorepeek-private-session-regression-label-v3`. Each included episode
binds the accepted song and clear type plus play side, play mode/type, difficulty, level, notes,
current score, all five judgments, typed miss/timing/combo values, and the typed previous-best
snapshot. Immutable v2 labels and suites remain readable and replayable; they do not acquire
performance truth retroactively. Replay requires every value present in the label generation to
agree with the catalog-constrained production observation. A review apply accepts only v3 labels,
publishes immutable label and suite objects, and atomically advances the active generation pointer;
the operator must confirm every new value before that apply.

```text
scorepeek-corpus diagnostic replay-video --video /absolute/input.mkv --profile gamescope-4k --output /absolute/new-diagnostic
scorepeek-corpus diagnostic verify /absolute/new-diagnostic
scorepeek-corpus corpus import-diagnostic --store /absolute/private-corpus-v1 --diagnostic /absolute/new-diagnostic --review-draft /absolute/review.json
scorepeek-corpus review show --draft /absolute/review.json
scorepeek-corpus review apply --store /absolute/private-corpus-v1 --draft /absolute/review.json --labels /absolute/operator-labels.json
scorepeek-corpus corpus replay --store /absolute/private-corpus-v1
```

`mise run corpus:test` replays every session in the active suite. It does not omit older sessions
when a new play is added. The old recording dataset and video-first sections below describe
read-only archived tooling and OCR-training inputs; they are not accepted by the current
capture-regression importer.

This document defines the M2 boundary between immutable private media,
offline corpus tooling, the future training/export pipeline, and the scorepeek
game-session core. Real media, extracted frames, complete labels, and replay
indexes remain outside the repository.

## Ownership boundary

- `scorepeek` is the Rust game-session core. It does not depend on corpus or
  training tooling.
- `scorepeek-corpus` is an offline Rust binary for private ingest and replay
  metadata. It does not run during a game session.
- The mise/uv-locked Python environment consumes explicitly exported canonical
  crop artifacts for offline OCR experiments and future training/export. Python
  is not a game-session runtime fallback.
- Only opaque fixture IDs, opaque group IDs or hashes, non-personal class
  labels, content hashes, schemas, and synthetic contract fixtures may be
  committed. A content hash is a reference, not permission to publish its
  content.

Every source binds one opaque capture profile and no canonical artifact.
Normalizer and layout bindings are selected later when a replay index maps the
observed source to the shared canonical frame contract. Corpus tooling does not
infer or model Wine, Vulkan, Gamescope, compositor, PipeWire, operating-system,
or capture-layer classifications from a profile ID.

## Complete recording dataset roots

This section and the remaining v1 recording-store sections are historical design records for the
read-only archive and OCR experiments. Their direct ingest, media extraction, generation sealing,
label authoring, replay-index, and replay-validation CLI routes were removed by ADR 0056. They are
not instructions for the active capture-regression corpus; use only the v3 diagnostic workflow
above for new sessions.

The preferred collection path imports one finished, self-contained Matroska
recording made from before game startup through final game shutdown. The raw
recording bytes are the durable dataset root; frame selections, canonical
frames, layout measurements, normalizers, labels, models, and replay artifacts
are derived and may be rebuilt later. See
[the Japanese operator workflow](recording-dataset.ja.md).

`recording import` accepts a strict `scorepeek-capture-context-v1` document and
derives the profile digest from that context plus the observed media contract.
It does not choose a baseline profile, attach layout, normalize pixels, or use
a Windows VM as a reference. The importer publishes immutable source, capture
profile, media-probe, and recording manifests. Reimporting the same recording
and context is idempotent.

`recording import` copies source bytes by default. Passing `--external` instead
publishes a private local locator and leaves the recording at its canonical
absolute path. The generation still binds only source SHA-256 and byte length;
the path is never part of a manifest or remote object. An operation hashes the
complete external file once when it must establish the declared source identity,
then consumes that same opened handle without a second unconditional local read.
Explicit verify and remote-transfer boundaries retain their complete checks.
Reimporting identical moved bytes updates only the locator.

```text
mise run corpus:recording:import -- --store /absolute/private/store --capture-context /absolute/private/capture-context.json /absolute/recordings/complete-run.mkv
mise run corpus:dataset:seal -- --store /absolute/private/store calibration-001
mise run corpus:dataset:verify -- --store /absolute/private/store GENERATION_SHA256
```

```text
mise run corpus:recording:import -- --store /absolute/private/store --capture-context /absolute/private/capture-context.json --external /absolute/recordings/complete-run.mkv
```

An exact calibrated recording profile can be normalized into the fixed RGB8
canvas without exposing observed pixels to a recognizer. Canonical extraction
emits the normalizer artifact, canonical extraction manifest, and bound PPM
frames together. The registry entry matches the capture-profile and FFmpeg
digests, container, codec, pixel format, geometry, time base, and explicit color
range/space/transfer/primaries. Unknown or merely similar profile contracts fail
instead of selecting a nearby transform.

```text
mise run corpus:canonical:extract -- --store /absolute/private/store --output /absolute/private/canonical PROBE_MANIFEST REQUEST
mise run recognition:inspect -- --extraction /absolute/private/canonical --extraction-sha256 FRAME_EXTRACTION_SHA256 --frame-id FRAME_ID
mise run recognition:crop -- --extraction /absolute/private/canonical --extraction-sha256 FRAME_EXTRACTION_SHA256 --frame-id FRAME_ID --output /absolute/private/crops
mise run recognition:music-select:crop -- --extraction /absolute/private/canonical --extraction-sha256 FRAME_EXTRACTION_SHA256 --frame-id FRAME_ID --output /absolute/private/music-select-crops
mise run recognition:integrated-context:crop -- --extraction /absolute/private/canonical --extraction-sha256 FRAME_EXTRACTION_SHA256 --frame-id FRAME_ID --output /absolute/private/integrated-context-crops
mise run recognition:title:dictionary:audit -- --catalog-store /absolute/private/catalog --dictionary /absolute/private/models/inference.yml
mise run recognition:title:model-export:requirements -- --catalog-store /absolute/private/catalog --baseline-dictionary /absolute/private/models/inference.yml --output /absolute/private/title-model-requirements
mise run corpus:music-list:observation-draft:inspect -- /absolute/private/music-list-observation-draft.json
mise run corpus:music-list:observation-draft:verify -- /absolute/private/music-list-observation-draft.json
mise run corpus:music-list:motion:measure -- --output /absolute/private/music-list-motion-artifact.json /absolute/private/music-list-motion-request.json
mise run corpus:music-list:motion:verify -- /absolute/private/music-list-motion-artifact.json
mise run corpus:music-list:motion:review-plan -- --output /absolute/private/music-list-motion-review-plan.json /absolute/private/music-list-motion-artifact.json
mise run corpus:music-list:motion:review-apply -- --output /absolute/private/reviewed-motion-request.json /absolute/private/music-list-motion-artifact.json /absolute/private/music-list-motion-review-plan.json /absolute/private/music-list-motion-review-decisions.json
mise run corpus:music-select:motion:review-plan -- --store /absolute/private-corpus-v1 --session-sha256 SESSION_SHA256 --video /absolute/original-session.mkv --output /absolute/private/music-select-motion-review.json
mise run ocr:model:fetch
mise run ocr:onnx:model:fetch
mise run ocr:spike -- --crop-artifact /absolute/private/crops --crop-manifest-sha256 CROP_MANIFEST_SHA256 --output /absolute/private/ocr-result.json
mise run recognition:title:spike -- --catalog-store /absolute/private/catalog --ocr-text OCR_TEXT --ocr-confidence OCR_CONFIDENCE
mise run recognition:title:provisional-candidates -- --catalog-store /absolute/private/catalog --output /absolute/private/provisional-hyper-title-candidates.json
mise run ocr:provisional-labels -- --review-disposition /absolute/private/review-disposition.json --review-disposition-sha256 DISPOSITION_SHA256 --review-plan /absolute/private/review-plan.json --review-plan-sha256 PLAN_SHA256 --source-artifact /absolute/private/source-motion-artifact.json --source-artifact-sha256 SOURCE_ARTIFACT_SHA256 --candidates /absolute/private/provisional-hyper-title-candidates.json --candidates-sha256 CANDIDATES_SHA256 --permission-status permission_not_recorded --expected-eligible-groups COUNT --output /absolute/private/provisional-hyper-title-labels.json
mise run ocr:parity:reference -- --crop-artifact /absolute/private/crops --crop-manifest-sha256 CROP_MANIFEST_SHA256 --candidates /absolute/private/parity-candidates.json --output /absolute/private/paddle-reference
mise run ocr:parity:run -- --model /absolute/private/models/inference.onnx --reference /absolute/private/paddle-reference --reference-sha256 REFERENCE_MANIFEST_SHA256 --crop-artifact /absolute/private/crops --catalog-store /absolute/private/catalog --dictionary /absolute/private/models/inference.yml --minimum-log-probability SCORE --minimum-runner-up-margin SCORE
```

Recognition requires the extraction SHA returned by canonical extraction and
validates `normalizer.json`, `manifest.json`, their typed canonical schemas and digest binding,
and the selected PPM's file and pixel hashes before constructing a
`CanonicalFrame`. A bare PPM or observed-frame extraction is rejected.
`recognition:crop` additionally requires the result predicate to pass, then
writes the layout-bound title, artist, difficulty, level, notes, and current
score PPM files plus a digest-bound manifest. The offline OCR command accepts
only an exact result or music-select manifest with its expected SHA-256 and the
registered normalizer digest; it does not accept a bare crop.
`recognition:music-select:crop` uses an
independent predicate and writes the selected title plus twenty geometric
visible-list slots. A slot can contain a separator, clipped row, or overlay;
downstream recognition keeps such content unknown instead of assuming every
slot is a title. OCR `--output` is create-only and retains the diagnostic JSON
for later replay without repeating inference.

`recognition:integrated-context:crop` keeps the established canonical layout and its historical
crop artifacts unchanged. A separately versioned layout binds the existing result artist ROI and,
for a stable music-select frame, the independently measured central artist, selected chart context,
and fixed active right-list title row. The create-only manifest reuses the canonical extraction,
normalizer, frame, and base-layout evidence. These crops are observation material; their presence
does not accept an OCR value, chart, song, threshold, or capture profile.

The provisional candidate export is fixed to the independently catalogued
single-play HYPER charts whose INFINITAS status is confirmed present. It excludes
search aliases and records every retained display variant with its source evidence,
lineage, revision, content digest, and rights statement. Provisional labeling
accepts only the digest-bound review disposition, its original review plan and
source motion artifact, and that exact candidate artifact. It rehashes every
selected 475x45 crop, requires every occurrence in its exact-pixel group to bind
stationary pair motion, runs the registered offline PP-OCRv6 model, and associates
only confidence-at-least-0.95 text whose versioned post-OCR comparison first applies
Unicode 17 NFC and removes U+0020, then, only when that exact tier has no candidate, folds U+FF01
through U+FF5E to ASCII and removes U+0020 and U+3000. A tier must resolve to one song
and one display string. Low-confidence text, absent catalog
keys, cross-song collisions, and display-string ambiguity remain reason-bearing
unknowns. The caller supplies the expected eligible-group count and explicit
permission status. These outputs are private provisional training inputs, not
human-confirmed labels, accepted holdout evidence, calibrated thresholds, or
redistributable data.

`recognition:title:dictionary:audit` verifies the immutable registered dictionary and the active
catalog, then emits catalog- and dictionary-digest-bound aggregate coverage. It splits non-search
variants by display kind and reports unsupported-character and CTC-timestep rejection counts
without exposing title strings. A rejected variant is never silently removed from inference scope.

`recognition:title:model-export:requirements` writes a create-only private manifest rather than
printing its character inventory. Starting from the registered baseline dictionary, it retains
every Unicode scalar that baseline entries can express, appends every missing scalar from every
active non-search catalog variant, and chooses at least the largest exact CTC alignment required by
that complete set. The non-blank order places U+0020 last because Paddle's `use_space_char` appends
it after reading the dictionary file. The catalog digest, baseline dictionary digest, resulting
scalar dictionary ordering, class count, timestep count, and
`scorepeek-title-ctc-f32-logits-btc-v1` tensor contract are bound together. This is an export
requirement boundary, not a trained model or a distributable bundle.

`ocr:training-input-manifest` accepts separately digest-bound v2 candidate, automated-label,
visual-audit, final-label, crop, and source artifacts. Its final-label input retains only reviewed
stationary standard music-list titles with `permission_not_recorded`; it must bind all six inputs.
The manifest assigns a song, rather than an individual crop, to one deterministic SHA-256 split, so
no catalog song can cross train, validation, or evaluation. It is private, provisional evidence:
it is explicitly not accepted holdout truth and cannot calibrate recognition thresholds. The
operator-supplied private artifacts are trusted inputs: this boundary detects accidental digest,
schema, binding, and split mistakes, but does not independently re-adjudicate every label against
each source artifact.

`ocr:title-model:prepare` consumes that training manifest, the create-only complete-catalog model
requirements, a separately digest-bound map from every group ID to its current absolute private
crop path, and the verified PaddleOCR v3.7.0 checkout. It rehashes each strict P6 crop, requires the
map to cover the training labels exactly, rejects any label outside the complete scalar dictionary,
and publishes a private create-only preparation directory. The directory contains a Paddle-format
dictionary, title-disjoint train/validation/evaluation lists, per-row crop file/pixel digest
sidecars, a derived Paddle config whose input width is eight pixels per required CTC timestep, and
aggregate complete-catalog coverage evidence.
The preparation independently recomputes every song's deterministic split and all declared split
counts. The derivation fails if the pinned upstream config no longer contains every expected source
value; the exported graph must still prove its actual shape through parity. U+0020 is represented
only through Paddle's `use_space_char` setting; it is not encoded as an ambiguous blank dictionary
line.

`ocr:title-model:pilot` starts from a digest-bound dictionary-mapped initializer and scores every
validation crop against every non-search title in an explicitly digest-bound current-catalog
candidate artifact. The vectorized Python CTC prefix trie follows the runtime scorer's exact-title
semantics; argmax open-text exact count is diagnostic only. It tries CPU candidates at 1, 2, then 4
optimizer steps with batch size 4 and a constant `1e-5` learning rate. A candidate is selected only
when it preserves the set of songs for which every available validation crop resolves correctly and
strictly increases that fully recognized song set. Correct-crop monotonicity within a song that was
already incomplete is not a selection requirement. The training input fixes the
label truth and may retain an older source-catalog binding; `--catalog-candidates` separately fixes
the current live search space and must match the preparation's catalog digest. The nested training
subsets are selected deterministically, while the selected checkpoint's actual bytes and observed
probes remain the evidence for a run. The selected checkpoint and its recipe, subset digests, and
candidate probes are published as a create-only v2 private artifact. This bounded pilot is not a
full training schedule and does not turn the provisional split into accepted holdout truth. Before
Paddle sees a selected training row, the runner verifies its sidecar digest and copies those exact
bytes into a short-lived private snapshot; validation and replay decode each path through a single
digest-checked read.

`ocr:title-model:export` revalidates the preparation, registered PaddleOCR source, and selected
pilot checkpoint before invoking the registered Paddle export entrypoint. It converts the produced
Paddle graph with pinned `paddle2onnx` at opset 11, with checker enabled, automatic opset updates and
optional optimizer dependency loading disabled. The Paddle graph, parameters, embedded dictionary
config, ONNX graph, and all exact hashes are published together as a create-only private artifact.
Training, export, and conversion subprocesses run in owned process groups with explicit timeouts
and bounded terminate/kill/wait cleanup on success, non-zero exit, timeout, or an interrupt that
arrives while the child is being spawned. Preparation, initializer, pilot, replay, and export
directory publication serialize their final existence check and rename with one parent-directory
lock. Export completes in a temporary work directory before verified files enter the short-lived
create-only publication staging directory.

`ocr:title-model:parity-reference` runs that exported Paddle graph on one preparation-bound crop and
publishes its exact input tensor, probability tensor, shapes, and CTC token orders. The Rust
`ocr:title-model:parity-run` command independently verifies the ONNX and inference-config hashes,
dictionary class count, dynamic input width and timestep relation, every output probability, and
the complete prepared dictionary token order, argmax, and collapsed token orders. This proves the exported graph contract; it does not select
the model or calibrate a recognition threshold.

`ocr:title-model:replay` compares the mapped initializer and either a v1 or v2 pilot checkpoint on
the complete provisional evaluation split and an explicit digest-bound private result-crop request.
Evaluation uses the same current-catalog CTC song-ID scorer and reports fully-correct song coverage,
crop-decision coverage, and runner-up margins; this lets an old checkpoint be re-evaluated without
retraining it. The two current result crops retain strict open-text and versioned exact
comparison-key diagnostics because their request has title truth rather than independent song-ID
truth. Every result artifact passes the same complete current-layout crop contract as ordinary
offline recognition input, and the replay records its extraction, canonical-frame, normalizer,
layout, and title-crop hashes. Result predictions are ordinary private diagnostic output, not
accepted holdout truth. Replay remains a title-disjoint generalization diagnostic; model selection
for the finite known corpus is made by the complete-corpus census below, not by validation or
evaluation alone.

`ocr:title-model:census` is the primary finite-corpus coverage measurement. It accepts one or more
explicitly named, digest-bound private Paddle checkpoints and scores all train, validation, and
evaluation crops against the same complete current-catalog candidate trie. Its create-only private
artifact reports total and per-split fully recognized song counts, every incomplete song, each
failing crop digest, the unique top song ID (or null for a tie), and the runner-up margin. Model selection for the
current 1,119-song corpus prioritizes the global count of fully recognized songs across the complete
corpus. Report gained and lost song sets, but do not require set inclusion: a local regression can be
accepted when global correct unique coverage increases. Wrong unique crop decisions remain failures,
and ties remain unknown. Validation and evaluation remain title-disjoint diagnostics
for generalization and overfitting; neither substitutes for complete-corpus coverage. Census is an
offline development measurement and does not calibrate the live absolute-score or runner-up-margin
rejection thresholds.

`ocr:official-onnx:census` applies the same complete-corpus song-identity measurement to pinned
official ONNX recognizers without removing or rewriting catalog titles to fit a model dictionary.
The legacy small parity path uses explicit `--model` and `--dictionary` files. Official-model census
instead takes a registered candidate's verified `--bundle` directory, explicit
`--bundle-model-id`, and native dynamic preprocessor; candidates include
`pp-ocrv6-small-rec-onnx-v1` and `pp-ocrv6-medium-rec-onnx-v1`. Inference is split into 128-crop process
batches, while the Rust decoder retains only one crop's input and output tensors at a time. Each
batch response is bound to its request digest and exact model, dictionary, preprocessor, width,
timestep, and input-tensor digests. The census publishes reusable open-text observations before
running exact comparison-key, absolute-Levenshtein, and normalized-Levenshtein search over every
catalog song. The create-only sibling defaults to `<output>.observations.json`, or an explicit new
`--observation-output` sibling; it therefore survives interruption or failure during catalog
search. A later run may supply that observation path and digest without model arguments to reproduce
all three searches without ONNX inference. Saved observations select their registered contract by
the exact model, dictionary, and preprocessor binding, so the legacy fixed-width small parity path
and the dynamic small census path remain independently replayable under the shared model ID. A
successful census also retains the same bytes inside its create-only result directory.

By default the command uses one scorepeek-owned
`.scorepeek-official-census-diagnostic` directory in the output parent. Its fixed marker, writer
lock, and single mutable `snapshot.json` bound ordinary retention to one latest run and less than
4 KiB of recording data. A first run completes the marker in a unique sibling staging directory,
syncs it, and publishes the store without replacing an existing path; later runs require the marker
exactly and reuse the same directory under the non-blocking writer lock. The recorder retains an open descriptor
for that exact directory inode, so renaming or replacing the outer pathname cannot redirect later
updates to an unrelated file. Each snapshot atomically records the current operation, total and
completed crop counts, model ID, completion state, and a fixed low-cardinality error type. It
contains neither crop paths nor titles; that allowlist supports low-cost progress and failure
diagnosis rather than a confidentiality requirement. `--diagnostic-output` selects another owned
latest-run store and `--no-recording` disables recording. An unowned path, active writer, or
diagnostic write failure is reported as dropped in the normal summary but does not change the census
result. The create-only census directory remains the result artifact and never includes this mutable
progress snapshot.

`ocr:short-title:probe` reproduces bounded observations for explicitly named one-character groups
without selecting a recognizer. It revalidates the private training input, crop map, registered
Paddle model, and complete catalog by SHA-256; evaluates the original crop plus two fixed diagnostic
foreground presentations over every one-character crop in that input; and performs complete
scoreable-catalog ranking only for the explicitly supplied target groups. Its create-only private
manifest records crop/model/catalog bindings, foreground geometry, input and output shapes, argmax
text, single-token ranks, target runner-up margins, and the number of catalog songs retained but not
directly scoreable at each target's timestep bound. The diagnostic `〆`/`x` alias and bias are
observations, not accepted catalog variants, runtime behavior, or calibrated thresholds.

`ocr:title-model:record-export` hash-records an explicitly selected Paddle model and ONNX graph
against one digest-bound preparation manifest. It reads and hashes the selected model outputs it
records, but does not rehash unrelated preparation files already accepted by their producing stage.
The record remains provisional, non-distributable, and unaccepted for runtime. It carries the
required output tensor contract and shape but deliberately marks the model's actual shape
unverified; a later Python-to-Rust parity/replay gate must establish that boundary before promotion.
Neither command starts training, downloads a checkpoint, chooses a device, or converts provisional
music-list labels into accepted holdout truth.

This distinction applies throughout the private workflow. Operator-provided artifacts receive only
the validation needed to catch ordinary selection, digest, schema, or result-invariant mistakes.
Content from a network, remote store, concurrent writer, or mutable filesystem boundary remains
independently verified because it can change outside that operator action; those checks are not a
trust judgment about the operator.

Stationary non-selected right-list rows may contribute provisional thin-title training evidence,
but retain their music-list origin. Selected rows, scrolling transitions, separators, and crops
whose title is hidden at either edge do not receive a complete-title target. Temporal stability
thresholds require a representative continuous-scroll recording; the current two frames are not a
calibration set.

The `scorepeek-private-music-list-row-observation-draft-v2` document makes each geometric slot
exactly one of `stationary`, `scrolling`, `selected`, `clipped`, `non_title`, or `unknown`, and
rejects a second annotation for the same frame and slot. Stationary and scrolling drafts must name
an adjacent decode index, report an integer full-row RGB L1 measurement, and independently identify
available versus locked/dimmed pixels and standard, INFINITAS-blue, or LEGGENDARIA-purple text.
An unlock-condition bar is explicit non-title content, never a hidden title. The inspection command
validates canonical shape, ranges, selected digests, and references. A downstream stage treats the
scorepeek-created crop and measurement artifacts as trusted; it does not repeat full-frame/crop
hashing or recompute L1 merely to reproduce the upstream result. The explicit observation-draft
verify command remains available when the operator requests a complete frame/crop/L1 audit; normal
downstream stages do not invoke it automatically. No state in this draft schema carries a catalog
title or complete-title label.
Locked/dimmed and non-standard color domains stay quarantined from standard-title training until a
versioned correction is measured.

The `scorepeek-private-music-list-motion-request-v1` contract binds each human-annotated adjacent
frame pair to both complete 21-crop artifacts. It requires exactly twenty semantic row annotations
for each frame and one explicit `stationary`, `scrolling`, or reason-bearing `unknown` motion state.
`motion:measure` never derives that state from pixels: it reads only the twenty required row crops
from each frame once while checking their declared digest and P6 shape, records all twenty RGB L1
sums and their checked aggregate, and creates a canonical
`scorepeek-private-music-list-motion-artifact-v1` without replacing an existing file. Unknown pairs
remain measurement evidence but cannot set a stability threshold; locked/dimmed,
INFINITAS-blue, LEGGENDARIA-purple, selected, clipped, separator, and unlock-condition annotations
remain explicit rather than being folded into title motion. The create-only review-plan command
consumes the selected artifact plus the referenced scorepeek crop manifests, and groups only rows
with the same declared pixel digest; it does not reread crop or canonical-frame pixels. The
create-only review-apply command accepts canonical plan-digest-bound partial human decisions,
validates their pair/frame/slot/current-annotation occurrences against the selected artifact, and
leaves omitted groups with their original annotations unchanged; initially these are usually
unknown. It neither reconstructs the plan nor re-adjudicates the preceding scorepeek-owned artifact.
The explicit motion verify command remains the opt-in full pixel and L1 audit. None of these stages
derives labels from luminance, color, OCR, or motion values.

The separate `music-select motion review-plan` command reconstructs 10 Hz frame motion around every
music-select interval in one active-suite video-replay session. It verifies the session, profile,
observation object, full video digest, packet-order PTS, and no-B-frame contract before seeking to
bounded frame runs. The session's full `capture/run.json` binding must match the current layout,
profile, and normalizer. Each selected decoded PTS is obtained from an independent bounded decoder
side channel and must equal both packet PTS and the retained observation timestamp. The create-only
`scorepeek-private-music-select-motion-review-draft-v1` records the observation sequence, source
packet index/PTS, screen class, and independent RGB L1/change metrics for the twenty-row list union,
active-list title row, and central title. A 500 ms context pad preserves rapid screen flicker and
selection transitions; overlapping pads are merged. Every span remains `unknown` with
`operator_review_required` until a human distinguishes `stationary`, `scrolling`, and
`selection_change`. Motion values and OCR agreement never create those labels, and the draft does
not implement dwell or become accepted corpus truth.

Apply human decisions without rewriting the measurement draft:

```text
mise run corpus:music-select:motion:review-apply -- --output /absolute/private/music-select-motion-reviewed.json /absolute/private/music-select-motion-draft.json /absolute/private/music-select-motion-decisions.json
```

The canonical `scorepeek-private-music-select-motion-review-decisions-v2` binds
`source_draft_sha256` and contains `decisions` with `span_id`, inclusive `first_sequence` and
`last_sequence`, and `state` (`stationary`, `scrolling`, `selection_change`, or
`screen_context`). A range expands to exact adjacent pairs and cannot cross an absent pair, a pair
touching non-music-select predicate context, or another decision. `screen_context` excludes a pair
whose retained predicates are both music-select but whose bound video visibly shows another
screen; it is stored as `unknown/operator_screen_context`, not motion truth. Omitted eligible pairs
stay `unknown/operator_review_required`; predicate-context pairs stay
`unknown/predicate_screen_context`. The create-only
`scorepeek-private-music-select-motion-reviewed-v2` artifact separately counts reviewed motion,
operator context, predicate context, and remaining pairs. It reports `complete=false` until every
predicate-eligible pair is either assigned motion or explicitly excluded, and cannot be used as
complete dwell-evaluation truth before then. ADR 0062 records the observed MODE SELECT false
positive that requires this fail-closed operator exclusion; review apply does not change the
production predicate. ADR 0063 fixes authoring precedence: visible active-selection identity change
is `selection_change` even when the list also moves; same-selection list translation or settling is
`scrolling`; unchanged selection and list placement is `stationary`. Central/background animation
alone is ignored. Motion values may direct visual inspection but never create a label.
The corrected complete application of that precedence is bound to draft
`f7d205cb38f9f29848f7b11261da0e0dee491fa172189d27997ce6cc68b36b5e`: 713 pairs are
`stationary`, 83 are `scrolling`, 30 are `selection_change`, 12 are operator-excluded screen
context, and 133 are predicate screen context. Its reviewed-set digest is
`e61341576367ee43ada17fcfb78c42f18a0cb4fe60a1cc1fb016c43b429a24a0`, with zero remaining
review pairs and `complete=true`. This establishes bounded motion-review truth only; choosing a
dwell policy and measuring stable-selection correctness remain separate work.

Evaluate frame-local accepted song IDs against that truth without changing the corpus or catalog:

```text
mise run corpus:music-select:dwell:evaluate -- --reviewed /absolute/private/music-select-motion-reviewed.json --output /absolute/private/music-select-dwell-evaluation.json
```

The evaluator verifies the reviewed-set, active-suite, session, observation-object, and exact
content-addressed catalog-generation bindings. It replays retained OCR strings through the
production music-select resolver; operator motion labels classify its temporal output and never
become runtime inputs. The create-only `scorepeek-private-music-select-dwell-evaluation-v2` report
contains no OCR or catalog strings and keeps all five truth denominators separate. It records
stability during nonstationary activity without calling same-identity scrolling a false song
decision. The corrected 100/200/300/500 ms comparison resets 4/4, 4/4, 3/3, and 3/3 selection
changes with prior stability and misses none. Stable nonstationary pairs are 23/17/16/15, while
stationary-run coverage remains 16/27, 16/27, 13/27, and 13/27. It selects no runtime policy because
motion truth does not label correct songs and therefore cannot measure OCR smoothing or wrong
acceptance. The report is descriptive motion/reset evidence, not a correct-song label set,
stable-selection accuracy result, or event authority.

Evaluate the bounded hold-and-replace candidate matrix against complete correct-song truth:

```text
mise run corpus:music-select:dwell:evaluate-correctness -- --reviewed /absolute/private/music-select-motion-reviewed.json --labels /absolute/private/music-select-correct-song-labels.json --output /absolute/private/music-select-correctness-evaluation.json
```

`scorepeek-private-music-select-correct-song-labels-v1` binds the reviewed-set digest and contains
one ordered entry for every maximal stationary run. Each entry repeats the exact `span_id`, first
sequence, and last sequence and uses either
`{"state":"song","scorepeek_song_id":"..."}` or
`{"state":"not_song_selection"}`. The latter retains stationary categories and filters as a
negative denominator instead of silently excluding them. The evaluator rejects missing, extra,
reordered, partial, or non-catalog labels.

The create-only `scorepeek-private-music-select-correctness-evaluation-v2` report replays complete
spans through the production hold-and-replace reducer. By default it compares 100/200/300/500 ms
dwell with 100/200/300 ms unknown grace; `--policy DWELL_MS:UNKNOWN_GRACE_MS` selects an explicit
bounded subset. It records confirmed correctness separately from `held_unknown` and `changing`
presentation, transition counts, song-run coverage, non-song final retention, stabilization
latency, wrong stable streaks, and per-run outcomes. The selected 200/200 ms policy is 705 correct /
0 incorrect / 35 unconfirmed over 740 stationary observations, covers the same 16/18 song runs,
and retains no song at the end of any of the nine non-song runs. Across the complete replay it has
ten held and five changing observations; three pending candidates clear on unknown without being
counted as grace expiry. Operator truth never becomes a resolver input or runtime
signal.

Python 3.12.13 and uv 0.11.7 are pinned by mise. `uv.lock` fixes PaddleOCR 3.7.0,
PaddlePaddle CPU 3.3.1, Apache-2.0 `paddle2onnx` 2.1.0, and their complete
dependency graph. `models/manifests/paddleocr-v3.7.0-training-source.json`
registers the official PaddleOCR checkout URL and immutable commit, its Apache-2.0
license, and SHA-256 bindings for the training and export entrypoints, PP-OCRv6
small recognition config, and source requirements. It is a reproducibility
record only: upstream code is neither vendored nor trusted as a pixel/layout
reference. `ocr:training-source:verify --source <absolute-checkout>` requires
that exact commit and all four file digests before a private training or export
run consumes it. The
`PP-OCRv6_small_rec` source manifest records the official archive URL, exact
archive and extracted-file sizes and SHA-256 values, Apache-2.0 license
reference, and compatible package versions. `ocr:model:fetch` downloads only
that registered archive, bounds and hashes it before extraction, rejects
unexpected tar entries, and publishes the verified three-file model below the
content-addressed `$XDG_CACHE_HOME/scorepeek/models` cache. The spike
always passes that verified local directory to PaddleOCR, so inference never
auto-downloads a model.

The separately registered `PP-OCRv6_small_rec` ONNX graph is pinned to an exact
official repository revision, byte length, SHA-256, Apache-2.0 reference, and
the same Paddle inference JSON/YAML digests. `ocr:onnx:model:fetch` publishes
only those verified bytes to the content-addressed model cache. The normal Rust CLI uses the same
cache base and globally fetches only the fixed PP-OCRv6-small three-file bundle before dispatch when
it is absent. The official source revision and Apache-2.0 license remain registered; cache deletion
causes reacquisition, so offline use requires one successful ordinary invocation while online.
The old `$XDG_DATA_HOME/scorepeek/models` store is not migrated or used as fallback.

Official-model comparisons use a separate bundle registry so they do not alter
the accepted small-model parity object. The registered candidates are
`pp-ocrv6-small-rec-onnx-v1`, `pp-ocrv6-tiny-rec-onnx-v1`, `pp-ocrv6-medium-rec-onnx-v1`,
`pp-ocrv5-mobile-rec-onnx-v1`, and `pp-ocrv5-server-rec-onnx-v1`; each candidate's exact registered
file set and native input/output contract are revision- and digest-bound together. For example,
`ocr:official-model:fetch -- --model-id pp-ocrv5-mobile-rec-onnx-v1` publishes
or re-verifies the complete v5 mobile bundle below the private model store. A registered bundle's
exact file set is model-specific; v6 bundles include inference JSON while the official v5 mobile
and server bundles contain only their ONNX graph and inference YAML. Bundle
publication holds a writer lock, removes only marker-owned interrupted staging,
fsyncs files and directory transitions, and permits at most eight bundles and
512 MiB total (192 MiB per bundle). An already present identical bundle remains
reusable at capacity. Registration and acquisition do not select the model or
establish recognition accuracy.

The parity candidate file is private canonical JSON. It contains at least two
exact catalog candidates and is shaped as follows:

```json
{"schema":"scorepeek-ocr-parity-candidates-v1","candidates":[{"song_id":"00000000-0000-0000-0000-000000000001","title":"TITLE A"},{"song_id":"00000000-0000-0000-0000-000000000002","title":"TITLE B"}]}
```

`ocr:parity:reference` first revalidates the canonical crop manifest and the
registered Paddle model. It retains the verified crop bytes and copies the
verified registered model bytes into a private temporary snapshot, so Paddle
does not reopen mutable source paths after verification. The pinned PaddleOCR
path then writes the exact preprocessed float32 tensor, the exported graph's
float32 output, argmax and collapsed CTC token orders, and constrained
candidate scores to a new private directory, with `manifest.json` written
last. The official graph exposes
post-softmax CTC probabilities rather than pre-softmax logits; this exact raw
graph output is the parity boundary.

`ocr:parity:run` requires the reference-manifest SHA-256, its bound canonical
crop artifact, the exact registered ONNX bytes and dictionary, one active
catalog, and explicit diagnostic thresholds. Rust reproduces the registered
3x48x320 BGR resize/normalize tensor from the verified RGB8 title crop; every
input value must match the Paddle reference within the fixed bound. ONNX
Runtime must also stay within the output bound and reproduce both token orders
and the parity reference's small candidate ranking.

After parity succeeds, the same ONNX probabilities are scored against every
exactly encodable non-search title variant in the identified active catalog.
The scorer shares prefixes in one CTC trie, performs no text normalization or
fuzzy matching, and returns `unknown` when any non-search catalog variant is
unencodable, the top song is tied, or the absolute log probability or runner-up
margin is insufficient. Unencodable variants are never silently removed from
the decision domain.
The emitted catalog digest, dictionary digest, preprocessor ID, and threshold
values bind the diagnostic result. These caller-supplied thresholds are not yet
calibrated acceptance policy; temporal agreement, independent screen context,
holdout evidence, and a supported profile remain unimplemented. The command
never emits free OCR text or an accepted title.

`recognition:title:spike` is a private-evaluation bridge for the open-text OCR
diagnostic. Its versioned post-OCR comparison first applies Unicode 17 NFC and removes U+0020. Only when
that exact tier has no candidate does it fold U+FF01 through U+FF5E to ASCII and remove U+0020
and U+3000. Case, non-ASCII-width compatibility characters, punctuation outside that range, and
other whitespace remain exact. An exact-tier candidate is never made ambiguous by the fallback.
Search-term aliases are excluded. Confidence below the fixed diagnostic bound,
no catalog candidate in either tier, or a comparison-key collision across songs returns unknown.
Even a unique candidate is not an accepted title because this spike has no raw
CTC-logit score, runner-up margin, temporal agreement, or independent screen
context.

The seal command includes every currently imported recording and writes a
canonical `scorepeek-recording-dataset-generation-v1`. Its SHA-256, rather than
the caller's human-readable dataset ID, is the reusable identity. A generation
binds every recording to its exact source media, source manifest, capture
profile, media probe, and recording manifest. Sealing checks the selected manifests,
references, object presence, and sizes without rehashing every trusted local object; the explicit
dataset verify command performs a complete local byte audit when requested.

Explicit push/pull commands synchronize a generation with private
S3-compatible storage. Objects use content-addressed keys and the generation
manifest is uploaded last. Push hashes the bytes received in remote staging, pull hashes each
downloaded remote object once and checks typed bindings, and remote reuse or explicit remote
verification hashes complete remote bytes rather than trusting an ETag. Push does not first repeat
a complete local dataset audit. Import never uploads. There is no
mutable latest pointer or delete command.

```text
mise run corpus:dataset:push -- --store /absolute/private/store --remote /absolute/private/remote.json GENERATION_SHA256
mise run corpus:dataset:pull -- --store /absolute/private/restored-store --remote /absolute/private/remote.json GENERATION_SHA256
mise run corpus:dataset:remote-verify -- --store /absolute/private/store --remote /absolute/private/remote.json GENERATION_SHA256
```

## Immutable ingest

The input request uses schema `scorepeek-private-corpus-ingest-v2` and contains
only an opaque fixture ID, an opaque session ID, and one opaque observed capture
profile ID. For example:

```json
{
  "schema": "scorepeek-private-corpus-ingest-v2",
  "fixture_id": "fixture-001",
  "session_id": "session-001",
  "capture_profile_id": "capture-profile-a"
}
```

Run ingest with an explicit absolute external store path:

```text
mise run corpus:ingest -- --store /absolute/private/store /absolute/source.media /absolute/request.json
```

Ingest streams the source into `content/<sha256>/source.media`, then writes the
canonical `scorepeek-private-corpus-source-v2` manifest to
`manifests/<fixture_id>.json`. A per-store writer lock serializes
recovery and publication. Newly created files and relevant directories are
synced before success is reported. The aggregate-only command result includes
capture-profile, source, and source-manifest SHA-256 values for downstream
binding.
Operator-selected roots, ancestors, sources, and read-only content may be symlinks; validation
applies to the resolved target. Create-only destinations remain no-clobber, and recovery or deletion
does not follow a substituted symlink entry. Filesystem permissions, ownership, ACLs, and retention
are operator responsibilities. Creation may use restrictive defaults, but scorepeek neither
validates nor guarantees Unix modes.

The same bytes and request are idempotent. An existing fixture ID cannot be
rebound to different bytes or metadata. Existing identical content remains
usable at capacity; new content is limited to 64 GiB per source, 1,024 source
objects, and 1 TiB total. Fixture manifests are separately limited to 1,024
files and 64 MiB total so content reuse cannot bypass the binding bound. These
are storage safety bounds, not recommended recording sizes.
All required capacity is checked before publishing a new content object. The
reuse path explicitly removes its complete staging copy and reports cleanup
failure instead of returning a successful binding.

Ingest deliberately does not inspect, decode, transcode, or extract the media.
Those operations use the separately approved and version-pinned tool described
below. The stored bytes are the reproducibility boundary even when the original
recording process was manual.

## Pinned media probe and frame extraction

The offline toolchain uses Shaka Project's static FFmpeg binaries at release
`n8.1.2-1`, containing FFmpeg 8.1.2. `mise.lock` pins the platform asset URL and
SHA-256 for FFmpeg and ffprobe; mise also verifies GitHub artifact attestations
during installation. The Linux x86-64 pair is about 92 MiB and fully statically
linked. The build reports GPL version 3, enables GPL/version3 components, and
does not enable nonfree components. It is an offline development/corpus tool,
not a Rust dependency or game-session bundle. This was selected over the roughly
649 MiB conda prefix, a nonfree Aqua build, rolling BtbN snapshots, and source
builds.

Probe a stored fixture into a new private manifest:

```text
mise run corpus:media:probe -- --store /absolute/private/store --output /absolute/private/probe.json fixture-001
```

`scorepeek-private-media-probe-v4` binds the canonical source manifest and
source object to the exact FFmpeg/ffprobe binary digests, video dimensions,
source time base, observed codec/pixel/color metadata, the sole video stream's
explicit index, and every FFV1 video packet's contiguous decode index and
integer PTS under `index_basis: ffv1_packet_order`. Media with zero or multiple
video streams, a non-FFV1 codec, or a packet without an integer PTS is rejected
rather than selecting a fallback implicitly. Probe
accepts only a self-contained Matroska container, streams its bytes to ffprobe
through stdin, forces the Matroska demuxer, and allowlists only the `pipe`
protocol. It therefore cannot follow a media-supplied network URL or secondary
filesystem path. Output is bounded to 64 MiB and 250,000 frames. Tool stdout
and stderr are drained with fixed bounds and every process has a ten-minute
timeout; errors expose only status and a stderr digest, not private decoder
text or paths.

Extraction takes a strict `scorepeek-private-observed-frame-extraction-v2`
request. It
repeats the fixture, source-manifest, and probe digests and supplies a non-empty
strictly increasing selection of `{frame_id, decode_index, source_pts}`. The
decode-index/PTS pair must match the probe exactly. FFmpeg also reports the PTS
of each actually decoded selected frame; count, order, and PTS must match the
packet-order probe before output publication. Before decoding, the tool
reloads the fixture's current canonical source manifest and requires the probe's
source object and capture-profile binding to match it exactly. Run extraction
into a new path:

```text
mise run corpus:media:extract -- --store /absolute/private/store --output /absolute/private/new-extraction /absolute/private/probe.json /absolute/private/extraction-request.json
```

At most 512 selected frames and 4 GiB of RGB payload are admitted. FFmpeg emits
RGB8 P6 PPM without frame-rate resampling. As with probing, the source is sent
through stdin with only the `pipe` protocol enabled and the Matroska demuxer
forced. The tool re-parses every PPM header, checks dimensions and exact pixel
byte count, records pixel-payload and whole-file SHA-256 values, and publishes
a canonical JSON extraction manifest that retains the observed capture-profile
binding and selected video-stream index. The extracted pixels remain observed
evidence at the source dimensions; this command does not normalize them or make
them a `CanonicalFrame`. An existing destination is never accepted, and
files/directories are synced before success. A parent writer lock
serializes recovery and publication. Recovery removes only staging and
incomplete destinations carrying exact scorepeek ownership markers. Atomic
no-clobber file and directory publication prevents an existing destination
from being replaced. The manifest's `ExtractorIdentity` uses FFmpeg 8.1.2, the
media-probe digest, and the canonical request digest.

## Immutable corpus generation

After ingesting all sources for one dataset generation, seal the complete
current fixture binding set under the same writer lock:

```text
mise run corpus:generation:seal -- --store /absolute/private/store generation-001
```

`scorepeek-private-corpus-generation-v1` contains an opaque generation ID and
the uniquely ordered set of every fixture ID plus canonical source-manifest
SHA-256 present at sealing time. The generation is stored by its own canonical
SHA-256 with fsync publication. Later ingests do not
rewrite it. A replay suite names this digest and must contain exactly one index
for every binding in that immutable generation; an arbitrary subset cannot
receive a corpus-wide validation summary. Existing identical generations remain
usable at the generation-store limit of 128 files, 256 KiB each, and 32 MiB
total; a new generation fails without changing older generations.

## Complete-label authoring

Author one complete-label document through the private store instead of writing
directly into `labels/`:

```text
mise run corpus:label:author -- --store /absolute/private/store /absolute/complete-label.json
```

The command bounded-reads and strictly validates the selected result,
music-select, or non-recognition shape, normalizes it to canonical JSON, and
publishes it as `labels/<sha256>.json` under the existing corpus writer lock.
Publication is idempotent, recovers only scorepeek-owned label
staging entries, enforces the existing 250,000-document/4 GiB label-store
bounds, and fsyncs the object and parent directory before success. Its
`scorepeek-private-complete-label-summary-v1` output contains only the opaque
frame ID, annotation revision, non-personal shape class, canonical byte count,
and label digest; it never returns labelled field values. Intrinsic validation
happens at authoring time. Exact frame, annotation, and screen-class binding is
checked again when a replay suite refers to the digest.

## Replay metadata

Before assembling a replay suite, generate each replay index from strict frame
metadata:

```text
mise run corpus:index:generate -- --store /absolute/private/store /absolute/index-plan.json
```

The `scorepeek-private-corpus-index-plan-v2` input names exactly one stored
fixture and its canonical source-manifest SHA-256. It binds the extractor
identity, one `canonical_frame` object, time base, and ordered frame metadata
already required by the replay contract. The canonical binding contains the
normalizer artifact SHA-256, canonical frame contract ID, and canonical layout
SHA-256 without making any of them properties of the capture profile. In place
of a caller-selected episode ID, each frame carries an
opaque `episode_sha256`. The generator uses that digest as the canonical
episode ID and rejects an episode group that reappears after a different group
has begun. Decode indexes must still increase strictly.

Generation checks the selected source-manifest digest/schema and each referenced label's required
schema plus exact frame, annotation, and screen-class binding before publishing canonical JSON to
`indexes/<replay_index_sha256>.json`. Publication shares the corpus writer
lock, uses fsync boundaries, recovers only owned index
staging files, and is idempotent for the same bytes. The index store admits at
most 1,024 objects, 32 MiB per object, and 4 GiB total. Its aggregate-only
summary contains the fixture ID, index digest, and frame and episode counts.
The generated index is directly usable as one entry in a replay suite; suite
assembly remains explicit because split-contract selection is a human dataset
decision. Index generation does not rehash the source media or re-adjudicate intrinsic label
contents already accepted by label authoring.

`scorepeek-private-corpus-replay-suite-v2` is the corpus-wide validation unit.
It contains an explicit `in_profile` or `profile_disjoint` split contract and
one or more `scorepeek-private-corpus-replay-v2` indexes. Each suite binds one
sealed corpus-generation SHA-256, and each index binds the exact canonical
source-manifest SHA-256 to one extractor identity, its version, exact
extractor-manifest and parameter hashes, its separate canonical-frame binding,
source time base, and a sequence of selected observed frames. Every frame
records:

- opaque frame and episode IDs;
- source PTS and a strictly increasing decode index;
- frame content SHA-256;
- non-personal screen class;
- `train`, `validation`, or `holdout` assignment;
- private session, play, and title group hashes;
- annotation revision and complete-label document SHA-256.

All indexes in one replay suite must name the same canonical frame contract and
canonical layout. Their normalizer artifacts may differ because each observed
capture profile owns its own mapping to that shared target.

Complete label values stay in the private store under
`labels/<sha256>.json`; the replay index carries only their immutable digest.
Each document uses the strict
`scorepeek-private-complete-label-v1` schema and is tagged as `result`,
`music_select`, or `non_recognition`. Result and music-select documents contain
their shape-specific mandatory fields as explicit
`known(value)`, `unknown(reason)`, or `not_applicable` states; mandatory fields
reject `not_applicable`. A non-recognition document explicitly distinguishes a
transition, negative scene, or unknown scene. Every document also binds its
opaque frame ID and annotation revision.

Replay validation bounded-reads the named label document, checks its canonical
SHA-256, schema, frame identity, annotation revision, screen-class/shape match,
shape-specific required fields, typed known values, result play-mode/type
compatibility, and `current_score <= 2 * notes`. Unknown counterpart values do
not cause a relationship to be guessed. The corpus-wide check also verifies
file type, size, filename digest, canonical schema, and intrinsic constraints for every
unreferenced label object. It never emits private field values. The labels
store is fail-closed at 64 KiB per document, 250,000 documents, and 4 GiB total.

Validation reads each named manifest and content-addressed media object from the
explicit store, verifies their bytes and duplicated index metadata, and rejects
duplicate fixture/frame IDs or non-canonical hashes. It also rejects
non-increasing per-source decode order and any session ID, episode, session
hash, play hash, title hash, or identical-frame digest assigned across multiple
splits anywhere in the suite. `in_profile` permits the same capture profile ID
in multiple splits so frozen holdout data can measure recognition within an
observed domain. `profile_disjoint` requires each capture profile ID to appear
in only one split even when its normalizer artifact differs, measuring transfer
to an unseen observed domain. The canonical frame contract and canonical
layout remain shared across the suite.
Before these checks, the suite's fixture/source-manifest set must exactly equal
its sealed generation. The title-group rule is the enforceable boundary for a
title-disjoint OCR holdout; it does not infer a title from private content.
Replay indexes must use the generation's unique fixture-ID order so the
canonical suite digest is invariant to caller traversal order.

```text
mise run corpus:replay:validate -- --store /absolute/private/store /absolute/replay-suite.json
```

The command outputs a dedicated
`scorepeek-private-corpus-replay-suite-summary-v2` result containing the sealed
generation digest, canonical replay-suite digest, opaque suite ID, index and
frame counts, selected split contract, and per-split counts. It does not emit
paths, media, complete labels, recognized values, or personal data.

## Catalog-independent synthetic title set

Render a deterministic synthetic title-crop set from a seed-only request:

```text
mise run corpus:synthetic:render -- --output /absolute/new/output-directory /absolute/synthetic-request.json
```

`scorepeek-synthetic-title-request-v1` contains only an opaque set ID, a
lowercase SHA-256 seed, and a sample count from 1 through 256. It deliberately
has no text, font, image, or catalog input. The versioned
`scorepeek-procedural-5x7-v1` renderer derives ASCII n-gram labels, glyph style,
gradient background, shadow, and bounded noise from the seed and sample index,
then writes fixed RGB8 512x96 P6 PPM crops plus a canonical manifest. An
existing output path is never overwritten. Files and the output directory are
world-readable (`0644`/`0755`) because this path contains generated data only;
the renderer does not read the private corpus store.

This baseline provides byte-deterministic, independently created renderer and
manifest contracts without adding a font, image, or media dependency. It is
not a claim that the limited procedural glyph domain is representative enough
to train the production OCR model. Expanding glyph coverage or adding an
external font still requires immutable provenance, a redistribution grant,
and the dependency approval described below. The repository's current lack of
a public license remains unchanged; this command does not itself grant rights
to redistribute scorepeek or its generated files.

## Not yet implemented

- replay execution against recognition code and labelled multi-recording or
  profile-disjoint evaluation of the current canonical layout;
- production synthetic variation and glyph coverage backed by an approved
  redistributable font or independently authored equivalent;
- Python training, holdout evaluation, ONNX export, and Rust parity gates.

Any media, image, training, or runtime dependency must be proposed with its
pinned version, license, alternatives, and host/bundle impact before addition.
