# Runtime diagnostics

Every `scorepeek run` invocation creates one ordered diagnostic stream whether or not video
recording is enabled. The invocation ID is the run ID; capture lifetimes use
`<run-id>-session-<generation>` IDs.

The stream records lifecycle, capture scheduling and failures, domain transitions, and
public event delivery in order. Runtime emits `runtime_event` records with
`scorepeek-runtime-event-v1` data, and formats core decisions as separate
`domain_transition` records with `scorepeek-domain-transition-v1` data. The two
record types share one `channel_sequence`. They are distinct from the public
Event API v5 sequence and from the stream's own record sequence. Runtime omits
individual raw screen, screen tick, and field observation records. Their
canonical sequence and pixels live in the separate recording.
Score startup failures prevent capture admission. The score health snapshot
reports migration detail gaps, retained backup path, recovered provisional
count, committed, failed, rejected, and pending write counts. A write failure
records `scores_failure` with its error type, cause, and those counts, then ends
the run with an error. Evidence JSON and player fields are not copied into this
failure record.

The runtime emits `domain_summary` after every 256 core inputs and at session or watcher
finish. `input_sequence` counts only values actually passed to the core domain coordinator;
runtime-owned watcher startup, overlay, and recording events do not consume a number.
`channel_sequence` advances for every logical runtime event, core transition and
observation input, including an observation omitted from the saved stream. Saved
records may therefore have gaps in `channel_sequence`; those gaps also advance
the frontend revision. Records emitted while processing a core input carry that
input's `input_sequence`; runtime events outside a core step omit it. The summary's
`inputs` and 256-input cadence use the same core-only count. The summary includes
cumulative no-op, transition, and output counts,
admitted frame and completed field counts, source sequence gaps, bounded screen and output-kind counts,
and a small state snapshot; it does not repeat canonical pixels or tick metadata.
The summary includes count, sum, and maximum microseconds for completed field processing,
end-to-end frame processing, and field queue wait. Missing timing values are excluded.
Session start and finish records carry the relative canonical locator when recording is enabled.
Session finish reports publication from the completed capture report;
`recording_summary` reports retained frames, elided ticks, bytes, and publication
status from the completed manifest. `runtime_run_summary` includes terminal recording
and diagnostic health.

The durable stream is
`$XDG_STATE_HOME/scorepeek/diagnostics/<run-id>/diagnostics.ndjson`. It contains lifecycle,
domain transitions, overlay child diagnostics, and exact public event payloads with their
`events.sock` enqueue outcome. Records use a diagnostic-local sequence, are limited to 1 MiB,
flush immediately without forcing filesystem durability. Persistence failure
degrades diagnostics only: the run continues, the in-memory ring remains available, and disk
writing is not retried in that invocation.

Capture lifecycle and error facts are written when they occur rather than being deferred until
session shutdown. Each backend also writes bounded rolling frame-timing summaries every 30
seconds and at session end with count, p50, p95, p99, maximum, and drop counters. Vulkan
summaries separately retain request-to-present, producer submit, present-call, post-present
producer-fence wait, consumer-readback, and total latency distributions. Producer-fence wait is
zero when the copy fence completes before `vkQueuePresentKHR` returns, so the present-call and
producer-fence stages do not count the same interval twice. Summaries also retain producer request, capture,
busy-drop, and coalesced-drop counters plus the selected consumer queue family, flags, global
priority, command-recording, and staging-map strategy. The `dropped` total contains producer
busy and coalesced drops; eviction from the bounded 600-sample latency window is not a frame drop.
Counters are present even before the first completed frame. The
`capture_session_identity` record contains the actual source contract, memory type,
stride, crop, normalization method, canonical output contract, and recognition resource
revisions. Public events expose only the session ID.
Capture diagnostic events carrying these stage distributions use the
`scorepeek-capture-diagnostic-v3` schema.

Catalog acquisition records `catalog_update` operations for URL resolution,
conditional fetch, ZIP extraction/digest verification, and atomic activation.
Attributes contain only the effective URL SHA-256 fingerprint, catalog digest,
mode, stage, status, and stable error type; raw URLs and response bodies are
not recorded. `$XDG_DATA_HOME/scorepeek/catalog/update-state.json` retains the
last successful check, active digest and URL fingerprint, optional HTTP
validators, and the latest typed failure. `scorepeek doctor` exposes those
fields without validator values. A background failure does not affect the
active invocation, and its failure state makes the next invocation retry
without waiting another 24 hours.

`$XDG_RUNTIME_DIR/scorepeek/diagnostics.sock` is independent of public `events.sock`. An observer
requests either live-only delivery or an explicit replay window before the server sends data.
Live-only delivery starts with the first record produced after the request. A replay receives the
records from the requested number of seconds that remain in the 128 MiB byte-bounded ring, then
live records. If capacity has truncated the requested window, the header sets `replay_truncated`
and reports `replay_available_us`, and the CLI warns on stderr while continuing with the available
suffix. A client that falls behind is disconnected and the observer exits nonzero instead of
resynchronizing.
Replay truncation and diagnostic persistence or socket failures reach the CLI as typed warnings;
the CLI chooses their stderr text. Writer failures remain recorded in the diagnostic stream and
health state, and are delivered once during normal runtime updates.

Private corpus replay reads the separate canonical recording contract directly. It does not
launch a runtime replay process or consume `diagnostics.sock`. The ordinary `scorepeek run` ring
remains 128 MiB and is bounded independently of canonical segment storage.

The runtime trace omits individual no-op core steps. A terminal summary also carries
the completed capture report's busy, rejected, failed and dropped worker counters.
These counters describe runtime admission of whole frames. Core owns the bounded OCR
queue and worker pool within an admitted frame; its completion order cannot reorder
domain commits, and its internal scheduling is not a canonical input or corpus oracle.
The `session_finished` record reports recording publication as `disabled`, `published`,
`partial`, or `failed`. Core's final session transitions are recorded and
projected before runtime emits `session_finished`.

```text
scorepeek diagnostic observe
scorepeek diagnostic observe --replay 30
scorepeek diagnostic inspect --latest
scorepeek diagnostic inspect --run-id RUN_ID
scorepeek diagnostic inspect --latest --format json
```

`diagnostic observe` stdout is streaming NDJSON. `diagnostic inspect` is a finite query with
human-readable output by default and one JSON document with `--format json`. The runtime validates the saved snapshot
before delivery and streams validated header and record values to the CLI for human formatting.
If stdout closes during delivery, inspection stops and exits with failure. `--latest` resolves
once to the active run, or otherwise the newest run. An incomplete active tail is
`tail_in_progress` and succeeds; an incomplete ended tail is partial and fails. Active state comes
from the run-directory lock rather than socket availability. Interior malformed data and sequence
gaps fail. The active-inclusive latest ten invocation
directories are retained; the next start removes the oldest completed invocation and its video.

`--record` adds selectively retained canonical video below
`sessions/<capture-session-id>/canonical/`, but TITLE pixels are always elided; `--record-all`
retains every canonical 10 Hz due tick, including TITLE, for calibration captures. Both publish a
`scorepeek-canonical-session-recording-v5` manifest with a session-local ID, fixed RGB8 shape,
tick-index and segment digests, completion state, and required `game_version` of
`identified(version)`, `not_observed`, `ambiguous`, or `observer_failed`. The canonical manifest
contains no capture profile, runtime binary or FFmpeg identity. The corpus canonical reader
verifies the recording directory and decodes every retained segment.
