# ADR 0112: Resolve MUSIC SELECT play type independently

## Status

Accepted

## Context

RESULT already observes an exact `SP` or `DP` string, but MUSIC SELECT had no play-type field and
the joint resolver removed every chart that disagreed with the stable RESULT type. That made an
unknown RESULT mode impossible to supplement from selection evidence and made a SELECT/RESULT
disagreement a fixed veto rather than an evidence comparison. The UI also had no authoritative
current-selection state independent of result acceptance.

The SELECT badge is fixed at canonical ROI `(45,130,100,80)`. PP-OCRv6-small reads the measured
samples as mixed-case `Sp` and `Dp`, which intentionally fails the strict RESULT parser. Independent
normalized-gray template measurements over four SP and four DP samples gave winner scores from
998,981 to 1,000,000 ppm and winner margins from 25,294 to 26,808 ppm. PLAY lane geometry differs
between SP and DP, but both share the fixed right-hand `GRAPH INFORMATION` band and `MAX SCORE`
label. Their amber pixels remain independent of note animation, background video, and graph target.

## Decision

- Integrated-context layout v5 registers the SELECT badge ROI, independently measured SP and DP QOI
  crops, a 980,000 ppm minimum score, and a 20,000 ppm winner margin. These two bounded crops are the
  only real captured image assets admitted to the repository by the operator for this decision.
- Screen-path layout v2 replaces lane-edge recognition with the common `GRAPH INFORMATION` band
  `(1505,0,410,24)` and `MAX SCORE` label `(1508,160,140,22)`. Both must independently meet their
  fixed amber-pixel minimums (7,000 and 800). Measured SP/DP frames have 8,537/1,073 qualifying
  pixels; the retained dim DP entry has 7,502/962. Loading and result-transition controls have zero.
  All screen predicates remain peers and overlapping classifications still fail closed.
- Integrated-context v5 also corrects the LEGGENDARIA `PLAYER 01` marker origin to `(760,605)`
  from its independently measured display position. Existing panel/fill/glyph thresholds are
  unchanged; the prior horizontally displaced crop failed to read the selected marker.
- Private label v5 may bind a PLAY span endpoint that the recording-time binary retained as raw
  `Unknown`; this permits calibration of the missing layout. Production replay must reclassify the
  endpoint as PLAY, while DECIDE and RESULT retain their complete raw-span requirements.
- A SELECT play type becomes stable only after at least two observations of one type and no
  observation of the other type in that semantic SELECT episode, including across song changes.
  SELECT and RESULT types are separate,
  equally capped evidence families. Neither removes the opposing catalog chart.
- Joint acceptance still requires RESULT evidence and all existing support, song-margin,
  chart-margin, numeric, attempt-path, and close-time gates. SELECT alone therefore never produces
  `AcceptedJoint`. An unknown RESULT type may be supplemented by stable SELECT type. When stable
  types disagree, all other evidence decides; a tie or insufficient chart margin remains unknown.
- `MusicSelectResolver` owns independent state over SELECT title/artist, selected
  difficulty, stable play type, and catalog hierarchy. It emits `music_selection_changed` only for
  first selection, song/chart replacement, retreat to unresolved, and episode end, with an
  episode-local revision. Both resolvers read the same immutable evidence, but neither the UI
  accumulator nor `MusicSelectionState` feeds evidence back into the joint resolver.
- Recognition observations advance to v20 and retain template scores and typed SELECT mode.
  Run events, the local observation socket, and its current-state snapshot advance to v9. The
  accepted `scorepeek-result-detected-v2` payload and its promotion gates do not change.

## Consequences

The runtime can show a typed SP/DP chart during MUSIC SELECT and use the same immutable observation
to supplement later result identity without promoting SELECT to domain authority. Mode conflicts
remain diagnosable through both stable values, family contributions, mismatch state, and the final
ranked chart. Canonical and numeric layouts and the numeric model binding are unchanged.
