# ADR 0038: Resolve result songs from retained simulation evidence

- Status: Accepted
- Date: 2026-08-24
- Complements: ADR 0034's full-catalog metrics, ADR 0036's shared recording path, and ADR
  0037's value-bearing local evidence

## Context

The counts-only recording field simulation proved that the production observer scored every active
catalog song, but it could not show what OCR observed or which song those values supported. ADR
0037 requires those values to be retained before resolver policy is selected.

The first value-bearing replay of `2026-08-17 19-25-31.mkv` retained 120 field observations and
305,760 candidate records. Its result evidence was stable after each initial animation frame:

- `ABSOLUTE EVIL` was observed as `ABSOLUTEEVIL` with artist `Yuta Imai`; both selected-candidate
  distances were zero and the title edit-distance margin was four;
- `ANEMONE` was observed as `ANEMON`; its selected title distance was one, normalized title
  similarity was `6/7`, and the next title distance was three; and
- the ANEMONE artist crop contained the truncated observation `d Team "HuΣeR X Yvya"`. The full
  catalog artist is longer, so adding title and artist edit distance incorrectly ranked other songs
  above ANEMONE. Its correct-candidate artist similarity was `20/43`.

The two initial result-animation frames had empty titles and artists. They must remain unknown
rather than being assigned to a short catalog title.

## Decision

The v1 result-song resolver is
`scorepeek-result-song-title-primary-artist-corroborated-v1`. It consumes one complete result field
observation and ADR 0034's complete candidates. It has result-screen song authority only; it does
not resolve music-select screens, charts, scores, clear types, temporal events, or support status.
The registered production field worker constructs this resolution together with its fields and
candidate set. Recording and Gamescope owners consume that same worker output; neither source owns
an alternate resolver call after the canonical-frame boundary.

Candidates are ordered by title minimum edit distance and stable `ScorepeekSongId`. The resolver
accepts only when all of these exact integer conditions hold:

- observed title and artist are both nonempty;
- the catalog contains at least a selected candidate and runner-up;
- the selected title minimum edit distance is at most one;
- selected title normalized similarity is at least `6/7`;
- the runner-up title edit distance minus the selected distance is at least two; and
- the selected candidate's artist normalized similarity is at least `2/5`.

The artist is a corroboration gate for the title-selected candidate. Artist distance is not added
to title distance and does not independently select a different song. Every failure returns a
typed unknown reason together with the selected and runner-up evidence when available. No threshold
relaxation, fallback, or guess is permitted.

`scorepeek-recording-recognition-simulation-profile-v2` extends each result episode with an exact
expected `ScorepeekSongId`. A v1 field-simulation profile has no expected song IDs; a v2 profile
requires one for every episode, and the recognition-simulation command rejects v1. A result episode
passes only after at least two frames contain its exact expected `CLEAR TYPE` and at least two
resolver acceptances contain its exact expected song ID. Any accepted different song is an
immediate typed simulation error. Initial unknown animation frames are retained and do not count.

The create-only local recognition artifact consists of:

- one bounded catalog JSON containing exact display, exact-comparison, and admitted folded strings
  for every active song;
- bounded NDJSON observations containing source sequence/PTS, exact OCR fields, all-song per-field
  metrics, typed resolver decisions/reasons, and episode expected values; and
- a manifest created last with run/profile binding, child digests, counts, bytes, and status.

The catalog file is limited to 16 MiB, observations to 256 MiB and 3,600 records, files are mode
0600, the directory is create-only, and files and parent directories are synced. Artifact capacity
or persistence failure is typed and cannot alter an already computed field or resolver value.
Pixels remain in the separately bounded diagnostic image store.

## Consequences

- The private recording now passes all three result episodes as two `FAILED` results for
  `ABSOLUTE EVIL` and one `CLEAR` result for `ANEMONE`, with 22 exact song decisions and two typed
  transition unknowns.
- These thresholds are a fail-closed first result resolver grounded by this recording. They are not
  release accuracy, title-disjoint holdout coverage, music-select resolution, event authority, or
  target-host performance evidence.
- A separately authorized live INFINITAS Gamescope run may now exercise this exact post-canonical
  result resolver, but cannot establish support until the remaining plan gates pass.
