# ADR 0106: Group recording staging by session

## Status

Accepted

## Context

Routine recording created capture, recognition, run-event, and canonical component directories in
separate purpose-first stores. Joined publication removed recognition and event staging but left
the capture directory behind, even though the same evidence had already been hard-linked into the
published joined session. The canonical staging location was also a separate implementation detail.
This made it difficult to tell which paths were temporary and which session they belonged to.

The watcher also atomically replaced `watcher-status.json`, but no repository consumer read it.
The TUI, observation socket, and recorded run-event stream already expose watcher state and session
lifecycle.

## Decision

With routine recording enabled, scorepeek creates one temporary tree per session at
`$XDG_STATE_HOME/scorepeek/recording-staging/<session-id>/`. Its direct children are `capture/`,
`recognition/`, `events/`, and `canonical/`. Component manifests retain the runtime session ID even
though their containing directories use purpose names.

After either complete or partial joined-session publication succeeds, scorepeek removes and fsyncs
the complete staging session tree. If component finalization or joined publication fails, the tree
is retained as bounded diagnostic evidence. Immutable published sessions remain under
`diagnostic-sessions/<session-id>/`.

Scorepeek no longer creates or updates `watcher-status.json`. Watcher state remains observable in
the TUI and observation socket; recorded lifecycle history remains in the joined run-event stream.
`run.lock` remains at the scorepeek state root because it enforces single routine execution rather
than recording evidence.

## Consequences

Temporary ownership and cleanup are visible from one session-first directory. Successful sessions
have one durable state artifact rather than a durable joined session plus redundant component
trees. Failed staging remains inspectable. Later session admission reclaims oldest entries from
each store as needed to retain fewer than eight existing generations and leave 4 GiB of the 8 GiB
aggregate allowance available for the new session. This admission policy does not introduce a new
live encoded-byte budget for an active canonical recorder.

Removing `watcher-status.json` is a local observability-contract change. There was no in-repository
reader or migration requirement; consumers use the existing live or recorded observation surfaces.

This supersedes ADR 0052 only for the watcher-status file. It supersedes ADR 0101 and ADR 0102 only
for routine component staging layout and successful-publication cleanup. Their capture, recording,
replay, memory, completeness, and publication contracts remain in force.
