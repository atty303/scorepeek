# ADR 0099: Treat MUSIC SELECT difficulty as current selection state

## Status

Accepted

## Context

ADR 0098 retained MUSIC SELECT difficulty as an episode factor. That is correct for title and
artist evidence, but not for the `PLAYER 01` marker: the marker describes the current chart selector
state rather than a historical vote. In saved target session
`run-1788255215-37773013-1050141-session-1`, the marker correctly observed the same song `X` while
difficulty changed `HYPER → ANOTHER → NORMAL → HYPER → ANOTHER`. Historical accumulation delayed
ANOTHER by about twenty observations and never projected the short NORMAL interval.

## Decision

Each selection epoch owns at most one `CurrentSelectionDifficulty`. It records the typed difficulty,
consecutive equal-known count, and first/last source sequence and monotonic time. One different
typed-known marker replaces the prior value immediately. An equal known marker increments the
consecutive count. Unknown, absent, no-candidate, and insufficient-margin marker observations are
gaps and do not clear the last known value.

Only the current value contributes SelectChart support, at 50 per consecutive equal-known
observation under the existing family cap of 300. Title and artist factors continue to accumulate.
Difficulty-only observations update the current successor when present, otherwise the incumbent,
otherwise a pending current value. Pending state is applied once to the first credible song epoch.
A new successor for a different song does not inherit the prior song's difficulty.

Select/result snapshot composition and retry inheritance choose the state with the newer source
sequence and never add difficulty streaks. Difficulty alone still cannot create a song, and it does
not resolve SP versus DP where the catalog remains otherwise ambiguous.

Typed debug transitions record only a changed value, pending application, epoch-target switch, or
reset. Equal-known repetition is retained in the current-state snapshot but does not create an
event. Debug events advance to `scorepeek-run-event-v5`; the observation socket and snapshot advance
to v5. Recognition observation v15 and public `scorepeek-result-detected-v2` do not change. Readers
retain run-event v2 through v4 and recognition v5 through v15.

The Resolver pane renders raw marker state and resolver current state separately, including the
consecutive-known count. Unknown raw state remains yellow while a retained current value keeps its
difficulty color.

## Consequences

Every decisive marker change is visible on its first field observation without changing song
support, song margin, or selection identity. Short difficulty intervals are no longer erased by
earlier repeated markers. The saved session remains a read-only failure oracle and is not added to
the corpus, label suite, or active pointer.

This supersedes ADR 0098 only for cumulative MUSIC SELECT difficulty support and its pending-state
merge semantics. Song/title/artist accumulation, factor projection, RESULT authority, thresholds,
and the public event contract remain unchanged.
