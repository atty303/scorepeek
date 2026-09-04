# ADR 0120: Persist scores as an independent event consumer

- Status: Accepted
- Date: 2026-09-04
- Supersedes: ADR 0119's persistence exclusion and coupling of public projection advancement
  to socket health and its fatal socket-initialization error; the roadmap's score/history persistence exclusion. UI remains out of scope.

## Decision

Ship `scorepeek-scores` in the existing binary. It consumes serialized public event v1 in producer
order, without a dependency on recognition, capture, internal run events or the scorepeek crate.
The public projection assigns identity once and fans out independently to the socket and scores
worker. A failed socket, including initialization/bind failure, cannot stop score persistence; a failed database cannot change recognition
or public delivery. Headless replay and diagnostic commands do not implicitly open a score database.

Add `emitted_unix_ms` to event v1 as signed UTC Unix epoch milliseconds. It is sampled once at
publication and retained unchanged by snapshots. It means notification time, not play start time
or an inferred achievement date. Existing monotonic clocks retain their separate meaning.

### Database instances

Normal `run` enables saving independently of `--record`. The default is
`$XDG_DATA_HOME/scorepeek/scores.sqlite3`, falling back to `$HOME/.local/share/scorepeek/scores.sqlite3`.
`--scores-db PATH` selects a file for the entire invocation; relative paths use the launch working
directory. Missing parents and database are created. `--no-scores` disables all score database I/O
and conflicts with `--scores-db`. Changing instances requires restarting run. An unavailable explicit
path never falls back to another database. The selected path and save health appear in run status.

An instance separates stored data, not player identity. Guest play using the same game account
still imports that account's SELECT and previous-best values. There is no player inference, game
account detection, database merge or automatic history deletion.

### Data authority and schema v1

Reuse the already approved bundled rusqlite dependency. `PRAGMA user_version` owns schema versioning;
unknown versions fail without recreation. SQLite uses WAL, synchronous FULL and a 250 ms busy timeout.
Schema version is checked under the initialization writer lock, so concurrent creators reuse the
committed schema. Initialization and each event update are transactional. Newly created parent components and
the database parent are synchronized. Committed data is the durability boundary.

`play_results` stores one confirmed RESULT per unique event ID, chart key, emitted/received times,
score/miss/clear and the complete public event JSON, including optional performance fields,
previous-best values and capture provenance. Repeated content with a different event ID is a
separate play. Provisional results never create a play.

`chart_bests` has one row per `(song_id, play_type, difficulty)`, including SELECT-only charts with
no play rows. It stores current-result cumulative best, previous-best cumulative best, current
SELECT supplements and their integrated best. No SELECT observation-history table exists.

Source columns `result_score/miss/clear`, `previous_score/miss/clear`, and `select_score/miss/clear`
contain nullable JSON facts. A fact carries `value`, source and origin (event ID, invocation/sequence,
emitted/received time, capture provenance and optional SELECT revision/observation ID). An absent
fact means unobserved; a SELECT fact with null value means explicit no record. Only current facts
are retained. `score`, `miss`, `clear` are SQL integer integrated columns; `score_origin`,
`miss_origin`, `clear_origin` retain their supporting facts. `presentation` is the latest available
public song presentation, independent of catalog availability.

Score maximizes, miss minimizes, and clear ranks are NO PLAY=0, FAILED=1, ASSIST=2, EASY=3, CLEAR=4,
HARD=5, EX HARD=6, FULL COMBO=7. RESULT clear spellings map from their existing wire vocabulary.
Current-result and previous-best sources accumulate their own best known values. SELECT replaces
each field with the latest known value, even if worse, allowing later observations to correct errors.
Unknown/not-displayed do not overwrite it; explicit no-record does. A null SELECT clear event only
clears live selection and does not erase stored supplements.

Integrated best is recomputed from the three current sources. SELECT corrections can reduce it
unless a result/previous-best fact still supports the old value. Unknown and no-record are not
numerical candidates. Ties retain an existing still-valid fact; otherwise source priority is result,
previous-best, SELECT. Fields may come from different revisions or plays. Never infer a common play
or achievement date; options stay in result history without separate option-based aggregates.

Inputs are the operator's own producer facts; schema/required fields and SQLite invariants detect
ordinary mistakes. Additive wire fields are ignored by typed projections and preserved in result JSON.
Unsupported schemas/contracts fail. The consumer tracks the last successfully handled invocation and
sequence during its lifetime to ignore duplicate/reversed inputs. New invocation sequence/revision
and wall-clock ordering are not compared against old invocations. No snapshot import/resume protocol
is introduced; event IDs deduplicate result reinsertion across database reopen.

### Failure and observability

The worker has 64 queue slots, an 8 MiB queued-byte budget and a 1 MiB record limit. Producer offers
never wait for database I/O. Initialization, write, worker, encoding or capacity failure latches a
save degradation and stops new admission for that invocation. Already admitted work may drain after
queue overflow. Failed transactions leave both history and best unchanged. Normal shutdown stops
admission and waits at most two seconds; timeout reports outstanding work without claiming it saved.
Uncommitted queued data can be lost on crash. There is no journal/retransmission/lossless guarantee.

The host owns diagnostics. Typed errors and health expose accepted, committed, duplicate, pending
and rejected counts, last committed event ID, stable failure classification, cause and flush outcome.
Run status shows the selected database, degradation and unsaved count. Opt-in internal event artifacts
include additive `scores_health` samples, separate from public events. Diagnostic recording failure
does not affect database commits; without recording no diagnostic artifact is claimed. No remote
export is added. Scores are operator data and are not committed to the repository.

## Verification boundary

Use isolated databases and synthetic public events for transactions, SELECT-only and later-RESULT
rows, corrections, partial values, no-record, source ties, ordering, deduplication, chart separation,
instance separation, schema mismatch, locks, worker capacity and bounded shutdown. Integration tests
use the production projection and recognition reducer, including a disconnected socket and an
unwritable database. Default workspace checks include the new crate. Developer-host tests do not
establish target-live performance or capture support.
