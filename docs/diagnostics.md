# Runtime diagnostics

Every `scorepeek run` invocation creates one ordered diagnostic stream whether or not video
recording is enabled. The invocation ID is the run ID; capture lifetimes use
`<run-id>-session-<generation>` IDs.

The durable stream is
`$XDG_STATE_HOME/scorepeek/diagnostics/<run-id>/diagnostics.ndjson`. It contains lifecycle,
recognition observations, overlay child diagnostics, and exact public event payloads with their
`events.sock` enqueue outcome. Records use a diagnostic-local sequence, are limited to 1 MiB,
flush immediately, and sync on important transitions and short intervals. Persistence failure
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

`$XDG_RUNTIME_DIR/scorepeek/diagnostics.sock` is independent of public `events.sock`. An observer
requests either live-only delivery or an explicit replay window before the server sends data.
Live-only delivery starts with the first record produced after the request. A replay receives the
records from the requested number of seconds that remain in the 128 MiB byte-bounded ring, then
live records. If capacity has truncated the requested window, the header sets `replay_truncated`
and reports `replay_available_us`, and the CLI warns on stderr while continuing with the available
suffix. A client that falls behind is disconnected and the observer exits nonzero instead of
resynchronizing.

```text
scorepeek diagnostic observe
scorepeek diagnostic observe --replay 30
scorepeek diagnostic inspect --latest
scorepeek diagnostic inspect --run-id RUN_ID
```

Stdout is NDJSON only. `--latest` resolves once to the active run, or otherwise the newest run. An
incomplete active tail is `tail_in_progress` and succeeds; an incomplete ended tail is partial and
fails. Active state comes from the run-directory lock rather than socket availability. Interior
malformed data and sequence gaps fail. The active-inclusive latest ten invocation
directories are retained; the next start removes the oldest completed invocation and its video.

`--record` only adds canonical video below `sessions/<capture-session-id>/canonical/`. Runtime
artifacts carry binding identities but no content digests. Corpus import computes video digests as
it reads the bytes for validation and transfer.
