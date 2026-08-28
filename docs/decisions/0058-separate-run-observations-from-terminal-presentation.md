# ADR 0058: Separate run observations from terminal presentation

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0052 only for its routine stdout and unusable-stdout contract

## Context

ADR 0052 made routine stdout a provisional `scorepeek-run-event-v2` NDJSON stream. That exposed
recognition evidence to machine consumers, but left an ordinary interactive run as an unreadable
event log and made the terminal result surface double as an observation transport. The stream is
not the accepted event API: it contains raw OCR values and resolver evidence which are prohibited
from the future `$XDG_RUNTIME_DIR/scorepeek/v1.sock` accepted-event protocol.

## Decision

One ordinary `scorepeek run` binds
`$XDG_RUNTIME_DIR/scorepeek/observations-v2.sock` after acquiring the ordinary-run lock. This
same-user Unix stream carries the existing provisional run records with a monotonic channel
sequence. A newly connected client first receives a `scorepeek-run-observation-snapshot-v1`
record containing current watcher/session state, the latest observation, and channel health. The
snapshot's `next_channel_sequence` is the first event not represented by that state; a client
discards any subsequently received live record below that boundary and detects later gaps by
sequence. The socket is distinct from the future accepted-event socket and grants no
stable-selection or event authority.

The application sends observation records through a capacity-64 non-blocking producer queue to at
most eight non-blocking clients. No client, a slow client, queue capacity, or a disconnected client
does not affect capture or recognition. Slow or partially written clients are disconnected;
producer drops, connected and disconnected client counts, and server degradation remain visible.
Socket bind failure is a startup failure. Under the already-exclusive run lock, scorepeek replaces
only a stale Unix socket and removes its socket at shutdown only if the filesystem identity still
matches the entry it created.

TTY stdout is a Ratatui alternate-screen presentation. It does not enable raw mode, so SIGINT keeps
its existing meaning, and it restores the cursor and screen on exit or unwind. It shows watcher and
session state, OCR observations, and a separate catalog-backed song resolution. An accepted song
shows every non-search display title, artist, song ID, principal resolver metrics, and runner-up;
an unknown resolution shows its typed reason and labels any leading entry only as a candidate.
Narrow terminals retain title, artist, and decision before IDs or detailed metrics. Non-TTY stdout
uses deduplicated human-readable state lines and never emits the observation records.

The song presentation is resolved from the same immutable session catalog evidence as the
candidate ID. An accepted ID missing from that evidence, or an artist presentation that is not
unique, fails closed. Full catalog tables and candidate lists remain in bounded recognition
artifacts. `--no-recording` continues to disable watcher status, diagnostics, and recognition
artifacts only; it does not disable the terminal result or observation socket.

Ratatui 0.30.2 with its Crossterm backend is the only new direct dependency. It is MIT licensed,
pure Rust, and adds no target host library or service requirement.

## Consequences

- Interactive runs expose useful state and recognized song information without mixing OCR and
  catalog authority.
- Existing stdout NDJSON consumers must connect to the observation socket.
- Observation delivery failure is measurable but non-interfering after successful startup.
- The public accepted-event API, stable-selection state, event deduplication, and future UI remain
  later M7 work.
