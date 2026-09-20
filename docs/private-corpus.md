# Private canonical session corpus

The attempt regression corpus replays complete, operator-reviewed recording sessions. Its pixel
authority is the canonical RGB8 1920x1080 stream produced during a `scorepeek run --record`
session. It does not ingest ordinary video, invoke a capture normalizer, expand segments into QOI
objects, or deduplicate frames by pixel content.

## Recording boundary

`scorepeek run` always saves one non-video diagnostic NDJSON stream. `scorepeek run --record` and
`scorepeek run --record-all` also start the canonical session recorder; neither enables a second
diagnostic format. Runtime QOI generation is absent, so canonical video is the session's only
retained frame authority.

Recording preflight requires a PATH-resolved FFmpeg that exposes
`libx264rgb`. The artifact records the executable digest and first version line. The logically
unbounded recorder uses one shared 1024 MiB memory account by default; use
`--record-memory-mib MIB` with either recording flag to change it. The TUI shows current, limit, high-water,
and dropped-frame values. A memory-limit admission loss, encoder failure, publication failure, or
shutdown timeout marks the recording partial but does not change screen resolution, attempt
finalization, or domain event emission.

One invocation lives at `$XDG_STATE_HOME/scorepeek/diagnostics/<run-id>/`. Structured evidence is
`diagnostics.ndjson`; optional video lives directly at
`sessions/<capture-session-id>/canonical/`. There is no digest staging or joined-publication step.

The canonical recorder indexes every 10 Hz due tick with original sequence, monotonic time, raw
screen, active semantic episode ID, and either `retained` or a typed intentional-elision reason.
It retains every `MusicSelect`, `DecideTransition`, and `Result` frame. It retains the session's
first and last ten ticks and ten-tick windows around all raw-screen changes, including entry to and
exit from `Unknown`. TITLE is the exception: ordinary `--record` always writes its tick metadata with
the `title` disposition and never retains its pixels, even inside those windows. Stable `Play`,
`ModeSelect`, and `Unknown` interiors are also elided.
Use `--record-all` for calibration captures that must retain every 10 Hz due tick, including those
stable interiors and TITLE. Its diagnostic stream records the effective `all` retention mode before capture.

`scorepeek-canonical-session-recording-v4` requires a final `game_version` state of `identified`,
`not_observed`, `ambiguous`, or `observer_failed`; only `identified` carries the complete version
string. Corpus sessions use `scorepeek-private-capture-session-v4` and preserve that state. The
ordinary importer accepts only canonical manifest v4 and corpus session v4; it has no v3 fallback.
Existing v3 sessions require the one-shot migration command from the separate temporary migration
commit. Because ordinary v3 recordings did not retain TITLE evidence, migration assigns
`not_observed` rather than inferring a version.

While that temporary commit is checked out, migrate only at the separately approved corpus
checkpoint:

```text
scorepeek-corpus corpus migrate-v3-to-v4 --store /absolute/private-corpus-v2
```

The command republishes the active suite's sessions and labels, updates matching import identities,
then atomically switches the active generation. Verify the migrated corpus before reverting the
temporary migration commit.

Retained frames are lossless RGB Matroska segments in tick-index order. Intentional sequence gaps
remain inside a segment; 600 retained frames, chronology reset, or session end closes it. The
realtime recorder does not hash video content. After semantic `session_finished`, the TUI shows
`finalizing`; saved manifest publication produces `recording_completed`, after which the session can
be imported while the invocation remains active.

## Import and review

Verify and import one completed capture session. This is where every segment is decoded and its
digest and frame count are checked. Each segment decode has a bounded two-minute deadline; timeout,
truncated output, or replay-observer failure kills and reaps the FFmpeg child before failing the
import:

```text
scorepeek-corpus diagnostic verify /absolute/diagnostic-run --capture-session-id SESSION_ID
scorepeek-corpus corpus import-diagnostic --store /absolute/private-corpus-v2 --diagnostic /absolute/diagnostic-run --capture-session-id SESSION_ID --review-draft /absolute/review.json
```

Import requires video and the session's saved `recording_completed` terminal record. A later partial
invocation does not invalidate a completed session. Import computes video digests while reading,
normally uploads segments to the configured remote, and removes successfully imported local video;
the invocation diagnostics remain until rotation. A non-video import receipt makes interrupted
post-publication video cleanup idempotently resumable. Identity is `run_id + capture_session_id`.
Import publishes source evidence as immutable digest-addressed objects. Volatile recognition
artifact and run-event schemas are not corpus storage contracts: import normalizes only the
sequence, source time, screen, fields, and decision needed by offline analysis into
`scorepeek-private-corpus-observation-v1`. Canonical segments, their tick index, capture bindings,
catalog evidence, and operator labels remain available to reproduce current recognition. The
review draft lists retained sequence identities; it does not create a separate image object per
tick.

Set `SCOREPEEK_CORPUS_S3_URL=s3://bucket/optional/prefix` and
`SCOREPEEK_CORPUS_S3_REGION=REGION` to keep canonical Matroska segments out of the local corpus.
`SCOREPEEK_CORPUS_S3_ENDPOINT` may select an S3-compatible HTTPS origin and
`SCOREPEEK_CORPUS_S3_PATH_STYLE` accepts `true` or `false`. Credentials use the standard AWS
process environment with a complete static, web-identity, task-relative container, or full
container-URI plus token-file credential set. Remote mode stores only segment objects in S3; all
metadata stays local, and no setting or locator file is written into the corpus. Corpus storage has
no aggregate byte or object-count quota. Import uploads each segment directly to its final SHA-256
key. It computes the complete local byte count and digest while filling signed-payload multipart
parts and aborts before completion on mismatch. An existing final object is reused only when HEAD
reports the declared size. Import performs no remote staging, readback GET, or server-side copy;
local-only use performs no S3 request.
The target bucket must also have an `AbortIncompleteMultipartUpload` lifecycle rule because no
process can clean up an upload ID after its own abrupt termination. Scorepeek aborts multipart
uploads for failures it observes while still running.

Regression truth uses only `scorepeek-private-session-regression-label-v5`. Each episode includes:

- a label-local `attempt_key` and optional earlier `parent_attempt_key`;
- ordered select, decide, play, and result sequence spans;
- an `accepted`, `abandoned`, `unlinked`, or `no_result` outcome;
- song/chart identity, clear type, numeric performance, and an explicit ordered distinct
  `play_options` list, including `[]` when no option was shown.

The existing `expected_result.play_type` is also SELECT play-type truth. `play_mode` must agree as
`single_play` with `single` or `double_play` with `double`; no separate SELECT label or alternate
conversion exists. Real full frames, complete labels, and generated corpus objects remain outside
the repository. The two independently measured 100x80 SP/DP templates under
`crates/scorepeek/assets/music-select-play-type-v1` are the sole narrow
repository-inclusion exception.

Every span endpoint must be retained on its expected raw screen, except that a PLAY endpoint may be
a retained raw `Unknown` when the operator label is calibrating a previously unrecognized gameplay
layout. Replay must classify that endpoint as PLAY before the truth can pass. Select, decide, play,
and result spans must be ordered. Every tick inside `DecideTransition` and `Result` spans must be
present, retained, and classified as that screen. Attempt keys are unique and a parent must name an
earlier attempt in the same label.

Apply the reviewed truth create-only:

```text
scorepeek-corpus review apply --store /absolute/private-corpus-v2 --draft /absolute/review.json --labels /absolute/operator-labels-v5.json
```

Partial sessions cannot become active regression entries. There is no alternate label reader,
converter, or archive path.

## Replay semantics

```text
scorepeek-corpus corpus replay --store /absolute/private-corpus-v2
# Explicit single-worker comparison only; this is not the default.
scorepeek-corpus corpus replay --store /absolute/private-corpus-v2 --text-workers 1 --memory-mib 2048
```

Replay losslessly decodes retained segment frames and supplies their original sequence and
monotonic time to the production screen-episode, field-recognition, attempt, RESULT-finalization,
and run-event reducers. Each active replay session exposes those production run events through an
isolated `diagnostics.sock`; replay connects live before publishing the session start and reduces
the ordered stream as it arrives. Sequence gaps, malformed records, or a disconnect before the
diagnostic run finishes fail the replay. The oracle retains only selection changes, confirmed
results, and aggregate counts rather than the full event stream. The isolated stream is socket-only:
it uses an 8 MiB byte-bounded ring and does not persist a second diagnostic NDJSON file. Frames are
streamed one at a time. Success and failure paths finish the diagnostic producer and reap the
observer and optional trace writer before releasing the active-session memory reservation.
Intentional gaps are not filled with
synthetic pixels: PLAY and MODE SELECT gaps continue their semantic screen, while a retained
UNKNOWN suspends until the next retained known frame or session end. A DecideTransition gap is an
invalid suite.

Replay and other segment consumers first use a verified local object. A missing segment is fetched
from the environment-configured S3 namespace into an anonymous temporary file, checked against its
declared byte length and encoded SHA-256, rewound, and passed to FFmpeg through stdin. The temporary
file is discarded after that decode and is not cached. Each active session prefetches up to two
manifest-ordered segments while current-segment decode and recognition proceed, and up to two
segments are materialized process-wide. Each uses four conditional Range GETs into non-overlapping
file offsets. A terminal `download_failed` is retried once after 250 milliseconds; missing objects,
permission failures, object replacement, size differences, and digest differences are not retried.
After all ranges finish, replay reads and hashes the complete assembled file; a prefetched file is
still never decoded before complete verification.

OCR may complete out of order but field evidence is committed by admission sequence. All sessions
share one text pool. Each scheduler step runs one `-threads 1` FFmpeg child for one segment, then
returns that session's ordered state to a FIFO so another ready session can use the slot before the
next segment. The automatic decoder count is the smaller of the session count and one quarter of
available parallelism, further constrained by memory; active session state is bounded at twice the
decoder count and all later sessions remain digest-only metadata. There is no fixed session limit.
The 2048 MiB default memory account bounds decoder reservations, active session state, and pending
field frames by backpressure; replay never drops them. `--memory-mib` accepts 256 through 8192, and
`--text-workers` accepts one through available parallelism. Replay stdout v4 additionally reports
local segment decodes, remote segment downloads, and downloaded bytes; it retains selected text,
preparation, and decoder concurrency; tracked memory high-water; decoder-consumer, preparation,
field-queue, and ordered-commit waits; raw classification, crop, text, numeric, join, and catalog
durations; stable per-session wall time; and corpus wall time. FFmpeg child wall time includes pipe
backpressure and callback consumption and is not a pure decode benchmark. Ordinary offline replay
always follows the available-parallelism-minus-four policy capped at twelve workers unless the
operator explicitly supplies `--text-workers`; a one-worker comparison uses `--text-workers 1`.

For every accepted label, replay requires exactly one ordered
`scorepeek-result-detected-v3` event with equal semantic payload, ordered play options, and normalized
parent relation. The runtime session ID, runtime attempt IDs, emission tick, and diagnostic metadata
are not truth. Missing, duplicate, extra, payload-different, play-option-order-different, and
parent-different events fail replay. Non-accepted outcomes require no event.

## Normalization verification

Observed-to-canonical correctness is independent of attempt regression. Verify it from a
profile-calibration artifact or an explicitly bound observed/canonical pair. `corpus replay` never
starts the normalizer; FFmpeg is used only to decode the already-canonical lossless segments.

SELECT best replay uses the same production observer and reducer. Per-session replay summaries
include `music_select_best_snapshots`; these never enter the accepted-result oracle. The current
field semantics and snapshot authority are defined in
[field semantics](field-semantics.md) and [Event API v3](event-api.md).

## Replay event traces

`mise run corpus:test --trace-dir DIR` retains production state/domain events as session-indexed
NDJSON in a new directory. Raw `field_observation` records are excluded: their OCR candidate
payloads remain in the existing recognition recordings. Retained events are written incrementally
from the same live diagnostic socket used by the replay oracle; the trace does not accumulate a
session event vector in memory. Each session uses a bounded nonblocking writer queue, so a slow
trace filesystem cannot stop diagnostic socket draining or another session's trace. Queue,
capacity, sync, or filesystem failure is reported only in that session's trace status and does not
alter result acceptance or the replay oracle. Headers identify the active corpus
generation, executable digest, selected-source fingerprint, registered text/numeric manifests and integrated/best layout digests.
Per-session `trace` summaries report path, written/total events, bytes and an optional error.
The budget is 256 MiB across the run; existing directories/files are not overwritten. No trace
output is created without `--trace-dir`; private traces must remain outside Git.

Every traced `raw_screen_observed` event includes the RESULT predicate evidence from the same
production classification pass, including warm-header counts and both panels' upper/lower anchor
counts, thresholds, qualification flags, and typed panel-side state. This permits anchor analysis
for frames classified as `unknown` without retaining pixels or enabling the separate diagnostic
frame recorder.

Compare interval starts, held identity, conflicts, content revisions and episode revisits using
source sequences. Endpoint SELECT labels do not assert a stationary span: inspect ambiguous
frames and retain interval labels privately before declaring a reset or repeat erroneous.
