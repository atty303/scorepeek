# ADR 0118: Retain SELECT intervals across missing evidence

## Status

Accepted. Supersedes ADR 0114's reset on unknown current-frame identity and its whole-state
resolver notification comparison. ADR 0117's raw difficulty predicate is unchanged.

## Decision

A selection interval and permission to adopt the current frame's best values are separate.
The reducer owns both. No dwell timer or unknown timeout is introduced.

- A frame can contribute best values only when its credible song set is exactly the resolved
  song and its known mode and difficulty agree with the resolved chart.
- Missing evidence without contrary identity evidence retains the interval and last published
  snapshot. It resets all field streaks and blocks publication until identity is confirmed again.
  Recovery requires two fresh equal observations per field. Equal recovered content does not
  create a new snapshot; changed content advances the same interval's revision.
- Any credible alternative song, or known different mode or difficulty, ends the old interval
  immediately, including ambiguous sets containing the old song and another song. The next
  resolved chart starts a new interval. A -> B -> A is a revisit even if B never resolves.
- Raw UNKNOWN suspends the interval, retains the last publication and resets field streaks.
  SELECT exit, session replacement and capture-generation replacement end the interval. Existing
  admission, ordering, resume-boundary and close-time gates still reject stale supplemental work.
- An unobserved excursion during an evidence gap cannot be distinguished from staying on the
  same chart. Without contrary identity evidence or an episode boundary it remains one interval.

Resolver notifications compare semantic state: activity, suspension, interval, identity/gate,
current difficulty/target, field stabilization, output and revision. Sequence/time-only updates
and resolved difficulty streak growth do not notify. Unresolved progress remains observable.
Internal observations stay fresh; connecting-client snapshots use the last published state,
matching event reduction. Existing public snapshot payload shapes remain unchanged.

The TUI formats typed state. Held identity is marked `held`; field rows show fresh stabilization,
and the output row identifies the previously published revision. It never merges values itself.
RESULT and music-selection event authority are unchanged.

## Replay diagnostics

`corpus replay --trace-dir DIR` optionally writes production events to session-indexed NDJSON
files under a new directory. Raw `field_observation` candidate payloads are excluded and remain
available in existing recognition recordings; the other production events are preserved verbatim.
The header binds the corpus generation, executable digest, selected-source fingerprint,
registered model manifests, integrated/best layouts and event schema. Event order and source sequences are retained.
Existing event fields expose identity gates; interval decisions use the existing diagnostic surface.

The shared run limit is 256 MiB. Existing paths are never overwritten. Trace status reports
written/total events, bytes and write/capacity errors separately from replay acceptance. Trace
failure cannot change emitted results, recognition or oracle success. No trace is written without
an explicit destination; no remote export is added. Private artifacts stay outside Git.

## Verification boundary

Use the same four-session corpus and registered models before and after the change. Endpoint
labels are not stationary per-frame truth. Classify actual revisits, content revisions and gap
recoveries from events and inspect ambiguous frames before adding private interval labels.
Synthetic lifecycle tests cover unobserved gaps, conflicts, suspension and delayed jobs. Trace
conformance, connecting snapshots and the 80x25 four-pane layout are separate gates. Development
replay does not establish target-live behavior.
