# Runtime diagnostics

Every `scorepeek run` invocation creates one ordered diagnostic stream whether or not video
recording is enabled. The invocation ID is the run ID; capture lifetimes use
`<run-id>-session-<generation>` IDs.

The durable stream is
`$XDG_STATE_HOME/scorepeek/diagnostics/<run-id>/diagnostics.ndjson`. It contains lifecycle,
recognition observations, overlay child diagnostics, and exact public event payloads with their
`events.sock` enqueue outcome. Records use a diagnostic-local sequence, are limited to 1 MiB,
flush immediately without forcing filesystem durability. Persistence failure
degrades diagnostics only: the run continues, the in-memory ring remains available, and disk
writing is not retried in that invocation.

Capture lifecycle and error facts are written when they occur rather than being deferred until
generation shutdown. Each backend also writes bounded rolling frame-timing summaries every 30
seconds and at generation end with count, p50, p95, p99, maximum, and drop counters. Vulkan
summaries separately retain request-to-present, producer submit, present-call, post-present
producer-fence wait, consumer-readback, and total latency distributions. Producer-fence wait is
zero when the copy fence completes before `vkQueuePresentKHR` returns, so the present-call and
producer-fence stages do not count the same interval twice. Summaries also retain producer request, capture,
busy-drop, and coalesced-drop counters plus the selected consumer queue family, flags, global
priority, command-recording, and staging-map strategy. The `dropped` total contains producer
busy and coalesced drops; eviction from the bounded 600-sample latency window is not a frame drop.
Counters are present even before the first completed frame. Generation
identity records contain both canonical capture/normalizer documents and their digests; public
events continue to expose only the digests.
Capture diagnostic events carrying these stage distributions use the
`scorepeek-capture-diagnostic-v2` schema.

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

Offline production-path consumers may create an isolated runtime root and use the same
`diagnostics.sock` request, header, ordering, gap, and terminal-completion contract. They complete
the live-only handshake before producing required records and consume the stream while the
producer is active. Isolated streams report their smaller 8 MiB ring in
`diagnostic_run_started`; the ordinary `scorepeek run` ring remains 128 MiB. Corpus replay uses
this isolated stream without creating a persistent `diagnostics.ndjson`, so concurrent replay
sessions cannot accumulate a second copy of their diagnostic payloads on disk. Neither ring is an
unbounded event collector.

```text
scorepeek diagnostic observe
scorepeek diagnostic observe --replay 30
scorepeek diagnostic inspect --latest
scorepeek diagnostic inspect --run-id RUN_ID
scorepeek diagnostic inspect --latest --format json
```

`diagnostic observe` stdout is streaming NDJSON. `diagnostic inspect` is a finite query with
human-readable output by default and one JSON document with `--format json`. `--latest` resolves
once to the active run, or otherwise the newest run. An incomplete active tail is
`tail_in_progress` and succeeds; an incomplete ended tail is partial and fails. Active state comes
from the run-directory lock rather than socket availability. Interior malformed data and sequence
gaps fail. The active-inclusive latest ten invocation
directories are retained; the next start removes the oldest completed invocation and its video.

`--record` only adds canonical video below `sessions/<capture-session-id>/canonical/`. Runtime
artifacts carry binding identities but no content digests. Corpus import computes video digests as
it reads the bytes for validation and transfer.
