# Event API v1

`scorepeek run` owns `$XDG_RUNTIME_DIR/scorepeek/v1.sock`. Connect with a Unix stream socket;
no request, handshake, subscription message, or ACK is required. The connection sends UTF-8 NDJSON:
one `scorepeek-event-snapshot-v1` record followed by `scorepeek-event-v1` records.
The endpoint serves the operator's local environment. File ownership, permissions and ACLs remain
operator-managed; the protocol does not use Unix mode as an acceptance or confidentiality claim.
The previous `observations-v11.sock` endpoint is removed, with no alias or second stream.

## Envelope and identity

Each live record contains:

| Field | Meaning |
| --- | --- |
| `schema` | `scorepeek-event-v1`, independent of diagnostic schemas |
| `invocation_id` | Identity of this process invocation |
| `sequence` | Public-only counter starting at 1, increasing by one for each publication |
| `event_id` | `<invocation_id>:<sequence>`; stable when that record appears in a snapshot |
| `emitted_unix_ms` | Signed UTC Unix epoch milliseconds sampled at event generation; notification time, not play start or past achievement time |
| `emitted_monotonic_ms` | Milliseconds since the invocation's output state was initialized, not wall time or play time |
| `capture` | Session context, or `null` outside a capture session |
| `event` | One of the kinds below |

Capture context has `session_id`, `capture_generation`, and `binding`. A live admitted session binds
`capture_profile_sha256`, `normalizer_sha256`, `canonical_layout_sha256`, `catalog_sha256`,
`model_sha256`, and `runtime_sha256` from its existing immutable recognition descriptor and acquired
capture lease. Model/runtime digests identify the existing registered text binding; they do not claim
to be a complete inventory of every numeric artifact. Unavailable binding is explicitly `null`,
never a guessed digest. Internal/headless inputs without a live descriptor can retain unbound context.
A retained result carries its original session context even after that session ends.

## Events and authority

| `event` | Additional fields and meaning |
| --- | --- |
| `result_detected` | `source_sequence`, `song`, `result`. Confirmed play; `result` retains `scorepeek-result-detected-v2` unchanged. Clears the active provisional result. |
| `result_provisional_changed` | `screen_episode_id`, `source_sequence`, `revision`, `state`. Existing resolved/withdrawn lifecycle; resolved content uses the same result v2 payload. No play/history authority. |
| `music_selection_changed` | `screen_episode_id`, `source_sequence`, `revision`, `state`. Existing selected/unresolved current-chart lifecycle. No play/history authority. |
| `music_select_best_observed` | `snapshot`: existing `scorepeek-music-select-best-snapshot-v1` payload, or `null` to clear the current best observation. Supplemental game record, not a play. |
| `status_changed` | `status`: current operational state described below. |
| `result_ingest_changed` | `ingest`: nullable RESULT persistence lifecycle. An ingest has an opaque `id`, `processing|persisted|failed` state, optional `result_event_id`, and an optional bounded reason. It is status, not another play. |
| `screen_state_changed` | `state`: nullable semantic screen episode. A state has `screen_episode_id`, `music_select|mode_select|decide_transition|play|result` screen, and `suspended`. This is presentation context, not recognition evidence. |

SELECT missing evidence and suspension retain the last publication without adopting new values.
Contrary identity evidence or SELECT exit clears it. A clear is sent only when a best publication
exists; it does not invent a new achievement. Partial values, explicit no-record values, unknowns,
revision/interval identities, and derived DJ rank keep their existing semantics.

`status` contains `watcher`, `capture`, `catalog`, `model`, nullable `scores`, nullable `recording`, and `last_session_outcome`.
Watcher values are `starting`, `waiting_for_source`, `ambiguous_sources`, `remote_unavailable`,
`catalog_unavailable`, `admission_rejected`, `session_active`, `session_finished`, and `stopped`.
Readiness is `not_ready`, `ready`, or `unavailable`: an admitted bound session makes catalog/model
ready; an unavailable catalog is explicit; an ended session no longer owns a ready runtime.
`last_session_outcome` is `null` until a session ends, then `stopped`, `source_ended`, or `error`.
It survives waiting/stopping and resets at the next session start. Detailed failure causes remain
diagnostics, not free-form public status strings.

## Snapshot and consumer state

The first record contains `schema`, `invocation_id`, `next_sequence`, `status`, and six nullable
slots: `latest_result`, `provisional_result`, `music_selection`, `music_select_best`, `result_ingest`,
and `screen_state`.
Non-null slots contain the complete original public event, including its event ID and provenance.
Only the most recent confirmed RESULT is retained; there is no history array.

To maintain the same state as the server:

1. Replace local state with the connection's snapshot. Treat retained RESULT as state, not a new play.
2. Require the next live record to have `sequence == next_sequence`; increment the expected value
   after each record. A different invocation starts a new stream identity.
3. Replace the slot associated with a domain event. A withdrawn provisional state clears
   `provisional_result`; a null best snapshot clears `music_select_best`. Confirmed RESULT also
   clears `provisional_result`.
4. Replace `result_ingest` on `result_ingest_changed`. Processing starts at a RESULT semantic episode when scores are enabled. Confirmed RESULT attaches its event ID; committed/duplicate DB success becomes `persisted`; write failure or a five-second timeout becomes `failed` with `persistence_failed`; recognition failure uses `recognition_failed`; interruption uses `interrupted`. A later success cannot overwrite failure. DECIDE or PLAY clears the slot.
5. Replace `screen_state` on `screen_state_changed`. Started/resumed semantic episodes publish an
   unsuspended state, UNKNOWN suspension publishes the retained screen with `suspended: true`,
   finalization publishes `null`, and closing publishes no visibility change.
6. Replace `status` on `status_changed`. A `session_active`, `session_finished`, or `stopped` status
   clears selection, provisional result, best, and screen slots, while retaining `latest_result`.

The server obtains snapshot and sequence boundary under the same lock as publication. It filters
queued records older than that boundary for each new client. Reconnect to obtain fresh current
state after a disconnect or detected gap. Consumers should reject unsupported schema versions. After validating the v1 envelope and sequence, unknown additive event kinds and fields may be ignored. Event IDs support deduplication, not recovery of missed history.

## Delivery limits and diagnostics

Delivery is live-only. There is no persistent event log, resume cursor, retransmission, or guarantee
that all RESULTs produced while disconnected will be recoverable. Process restart clears retained
state. The in-process scores consumer (ADR 0120) saves confirmed plays independently of socket delivery,
but does not add replay or lossless guarantees to this endpoint.

At most eight clients are served, with a nonblocking producer queue of 64 records. Each event and
snapshot is at most 1 MiB including its newline. A slow client, partial write, or failed write closes
that client alone. Producer overflow invalidates existing connections even if no later event arrives;
new connections start from the latest snapshot and never continue the broken stream.
An oversized record/snapshot or failed worker makes the public channel unavailable for the rest of
that invocation; it neither truncates fields nor stops recognition. Socket initialization failure disables public delivery while recognition and the independent
scores consumer continue; run status shows the failure.
The runtime replaces only a stale Unix socket and removes only its own inode on shutdown.

Raw OCR, candidate lists, resolver scores, processing timings, recording paths and history arrays
are excluded by typed projection. The TUI and headless replay retain the internal run events.
With `--record`, internal event records include an additive `event_api_health` diagnostic sample
with invocation ID, client/drop counts, oversized-record count and stable failure classification.
Samples reflect health at that internal publication, not an exact client-disconnect timeline.
The existing local diagnostic recording limits and completeness rules still apply. Without recording,
live channel health remains available to the application's existing status presentation. No remote
export or new recording default is introduced.

Score persistence is documented in [ADR 0120](decisions/0120-persist-scores-as-event-consumer.md).
Projection identity continues advancing after socket delivery is disabled; the scores consumer remains
independent. Database health is not part of the public wire; internal recordings may include
`scores_health` samples.
