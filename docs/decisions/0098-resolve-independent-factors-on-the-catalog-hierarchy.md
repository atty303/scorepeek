# ADR 0098: Resolve independent factors on the catalog hierarchy

## Status

Accepted

## Context

ADR 0097 accumulated support after expanding every observation into catalog chart candidates. That
made the number of populated families part of authority: one wrong MUSIC SELECT title could receive
several correlated title/chart contributions, while a later RESULT artist and notes observation
could not apply its chart evidence to a song candidate introduced in another frame. A global
runner-up also allowed a sibling SP/DP chart to hide the strongest competing song. The saved target
session `run-1788248141-530814846-1005386-session-1` exhibits this failure for operator-confirmed
`∀ / SP HYPER / notes 1136`; it remains a read-only failure oracle and is not corpus input.

The three-pane TUI also discarded MUSIC SELECT field values before updating its resolver snapshot,
and its mostly unstyled tree did not expose enough typed evidence and gates to explain a failed
promotion.

## Decision

Episode accumulators retain independent song and chart factors. Song factors are select title,
select artist, result title, and result artist. Select difficulty and RESULT difficulty, notes, and
advisory level are retained independently of the candidates present in their source frame. They are
applied only when a song candidate exists, so chart evidence cannot generate song identity. Score
compatibility remains final semantic validation and is not ranking support.

Foreground lexical and geometry evidence from the same active-title crop form one `select_title`
family. Their candidate delta is the maximum, never a sum. Raw OCR, scalar count, foreground bounds,
occupancy, and edge contact remain separate diagnostic facts. Candidate/family raw sums remain
`u64`; observations from distinct source sequences accumulate, and family-wide proportional
normalization preserves ratios above the cap of 300.

Summary projects the factor accumulator onto the catalog hierarchy. Each chart receives its song
factor plus matching chart factors. The song projection uses the best chart beneath each song.
`runner_song` is the best chart under a different song, while `runner_chart` is the best sibling
chart under the selected song. Acceptance requires support 260, different-song margin 50, and
sibling-chart margin 50. Song-only resolution is `song_projected`; insufficient different-song
margin is `conflict`. Resolver authority retains the complete hierarchy as typed input across the
capture adapter. Run-event artifacts and sockets construct only a top-eight diagnostic projection;
they never materialize the full hierarchy as JSON. Select/result snapshots and retry
inheritance combine factor state once. A known MUSIC SELECT difficulty without song evidence waits
for the next credible song observation in that selection epoch.

MUSIC SELECT snapshots are handed to the attempt only at semantic finalization, after every field
job admitted before close has completed or failed. Closing itself does not copy the accumulator.

RESULT remains provisional until semantic close and admitted-field drain. Path, clear, numeric, and
identity gates are then finalized; confirmation precedes the attempt's sole public
`scorepeek-result-detected-v2` event.

Debug events advance to `scorepeek-run-event-v4`, the observation socket and snapshot to v4, and
recognition observations to v15. Resolver transitions retain hierarchical runners, top candidates,
normalized family contribution, and gate state. Readers keep run-event v2/v3 and recognition v5
through v14. The public socket and result schema do not change.

The TUI keeps the fixed three-pane layout. MUSIC SELECT field observations now update its typed
snapshot. Field evidence is retained across ticks and suspension but cleared on another semantic
episode or session boundary. Resolver shows field age, raw/typed observations, top and hierarchical runners, chart
identity, family contribution, attempt path, and all gates. A shared semantic palette is used:
green for healthy/accepted, cyan for active/provisional, yellow for pending/unknown, red for
failure/conflict, dark gray for inactive/absent, and white for raw OCR. Difficulty and clear type
retain their domain-specific colors. Text labels and gate symbols remain authoritative when color
is unavailable.

## Consequences

An earlier HYPER marker can constrain a song introduced later by RESULT artist evidence, without
letting HYPER create that song. Repeated non-discriminating chart evidence cannot change the song
margin. A same-song DP sibling no longer masks a different-song collision. The target failure
oracle resolves `∀ / SP HYPER / 1136` without rewriting raw `A` or requiring a RESULT title.

Policy authority remains provisional until the replacement private corpus passes session-disjoint
zero-wrong joint and event gates. This supersedes ADR 0097 for candidate-preexpanded chart support,
separate lexical/structural title voting, global runner-up semantics, and the v3 debug surfaces.
