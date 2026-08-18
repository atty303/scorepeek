# ADR 0016: Use stationary music-list rows as result-title evidence

## Status

Accepted

## Context

Collecting many play-result screens is slow, while one music-select recording can expose most of
the operator's owned songs. The right-side list and result screen appear to use the same thin title
rendering, but the selected central title is visibly larger and heavier. Scrolling also creates
transitional rows, and the list UI can obscure either edge of a title.

In one normalized recording, `ABSOLUTE EVIL` was observed both in a result at source PTS 190000
and in music-list slot 10 at source PTS 110000. Without scaling, a translated neutral foreground
mask for the intact `BSOLUTE EVIL` suffix had intersection-over-union 0.956. This strongly supports
shared glyph rasterization for that observation, but does not prove universal texture identity
across every character, selection state, or game revision. The same frame directly showed that the
selected list row shifts left far enough for the current generic list ROI to clip its first glyph,
and that the game UI itself obscures the right side of long rows.

## Decision

- Stationary, non-selected right-list rows may provide provisional private training observations
  for the thin result-title recognizer. Every observation retains its screen origin so result-only
  holdout and cross-origin evaluation remain possible.
- The selected central title is a separate rendering domain. A selected right-list row is also
  excluded from the initial shared-row corpus because its horizontal placement and color state
  differ, even when its glyph design appears similar.
- A full catalog title is never assigned to a crop whose visible text is clipped or obscured.
  Such rows remain `clipped` or `unknown` until a separately observed complete row supplies the
  full label; inferred hidden suffixes are not training targets.
- Scrolling and settling are distinct temporal states. A row is not admitted merely because a
  single frame passes the music-select screen predicate. Collection must retain adjacent frames,
  reject vertical motion and transition states, and deduplicate repeated stationary observations.
  Numeric stability thresholds will be calibrated from the planned continuous scroll recording,
  not from the current two representative frames.
- Result recognition remains the primary acceptance gate. Music-list training value cannot replace
  result-screen holdout, independent result context, or zero-error accepted-result requirements.

## Consequences

- One deliberately paced full-list recording can supply broad title-font coverage without requiring
  thousands of plays.
- The recording procedure should pause after each scroll step long enough to expose adjacent settled
  frames; continuous fast scrolling is still useful negative data but not positive labels.
- Corpus tooling needs explicit row states for stationary, scrolling, selected, clipped, non-title,
  and unknown observations before automated provisional-label generation.
