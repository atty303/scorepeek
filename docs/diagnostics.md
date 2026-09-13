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

`$XDG_RUNTIME_DIR/scorepeek/diagnostics.sock` is independent of public `events.sock`. A connection
receives one header, the available portion of the 128 MiB byte-bounded ring, then live records. A
client that falls behind is disconnected and the observer exits nonzero instead of resynchronizing.

```text
scorepeek diagnostic observe
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
