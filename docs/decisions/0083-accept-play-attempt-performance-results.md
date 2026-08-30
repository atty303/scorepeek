# ADR 0083: Accept play-attempt performance results

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0078 only for provisional savability, result-event acceptance, and the v1 result payload
- Complements: ADR 0068, ADR 0077, and ADR 0079

## Context

ADR 0078 emits one provisional result event from a stable result-local song and clear type plus an
accepted catalog chart and current score. It fixes `savable=true`, 1P, and SP for the admitted
corpus slice. The application now has a play-attempt reducer that separately observes selection,
gameplay, result linkage, conflict, and retry inheritance. Treating savability as another result
image field would duplicate the stronger application fact that a play was actually observed and
its stable result matched the selected or retry-inherited song.

Retained result QOIs also contain fixed performance fields that are useful downstream but are not
part of the current result payload: five judgement counts, miss count, fast and slow counts, combo
breaks, and the previous-best clear, score, and miss snapshot. The score breakdown provides an
independent exact check of the recognized EX score. Previous-best and the remaining performance
fields are useful reference values but must not turn an otherwise accepted play into an unknown.

## Decision

- Replace the provisional `scorepeek-result-detected-v1` payload with
  `scorepeek-result-detected-v2`. Remove `savable`; carry the accepted attempt ID and optional parent
  attempt ID.
- Accept a play attempt for a result event only when gameplay and result were both observed and the
  stable result song confirms the selected or retry-inherited song. A missing decision transition
  is retained as path evidence but does not prevent acceptance. Missing gameplay, abandonment,
  conflict, and an unlinked result do prevent the event.
- Require exact non-negative `pgreat`, `great`, `good`, `bad`, and `poor` values. The event requires
  `current_score == 2 * pgreat + great`; every judgement is bounded by chart notes. Do not require
  the sum of judgement values to equal notes because failed-play and POOR semantics have not been
  established as that invariant.
- Retain miss count, fast, slow, and combo break as typed supplemental values. Each is `known`,
  `not_displayed`, or `unknown`; none blocks the event. A displayed dash on a failed result is
  `not_displayed`, not a guessed zero.
- Retain previous-best clear type, score, and miss count as independent typed reference values.
  They additionally admit `not_played`. A recognized `NO PLAY` marker makes all three values
  `not_played`. Previous-best values never affect current result acceptance.
- Record every raw OCR observation, parsed typed value, and score-breakdown rejection in the
  bounded recognition and run-event artifacts. Public result payloads contain typed values, not raw
  OCR, crops, radar data, ranking names, or diagnostic reasons.
- Author new private regression labels and suites as v3 while keeping immutable v2 generations
  readable. `play_options` remains absent until multiple independently measured option displays
  establish its vocabulary and layout.

## Consequences

- Accepted-result authority moves from a fixed savability assertion to an observed application
  path. Result-only, conflicting, and missed-gameplay observations remain diagnostic evidence.
- Downstream consumers receive an internally checked score breakdown plus bounded optional play
  detail and previous-best context without depending on OCR strings.
- More result crops and model invocations increase result-screen observer cost. Offline replay must
  remain complete, and target 10 Hz cadence and busy skips remain an explicit later qualification
  gate rather than being hidden by queue or sampling changes.
- DJ level, score deltas, percentages, and new-record status remain derived values. Radar, rival
  ranking, same-rank average, graph, BIT, dead point, and play options remain outside this payload.
