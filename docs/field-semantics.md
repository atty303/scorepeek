# Field semantics and applicability

This document is the v1 source of truth for deciding whether a field is
`known`, `unknown(reason)`, or `not_applicable`. It is based on the inspected
upstream snapshot recorded in `research.md`; every adopted upstream release
must replay these predicates before it can replace the active schema adapter.

An unmatched recognizer never means that a field is absent. `not_applicable`
requires the positive predicate below, including an explicit blank/absence
template where the UI can legitimately omit a value. If the predicate itself
cannot be established, the field is `unknown` and no detected event is emitted.

## Result

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| result state | Precondition for every result event | Unique result-screen anchor; transition, overlay, and unknown screen classes do not count |
| savable | Always for a result candidate; it must be `known(true)` to emit a result | Unique match between independently calibrated savable and non-savable anchors. No-match, unknown background, partial corruption, or ambiguity is `unknown`, never `false` |
| playside | Always for a result candidate | One unique playside anchor; no-match or ambiguity is `unknown` |
| loveletter, dead | Always after playside is known | Profile-specific positive and negative masks; a failed positive match alone is not `false` |
| play mode, difficulty, level, notes, song | Always | Unique resource/template match and catalog consistency; song also follows the OCR agreement policy |
| play speed | Only when the versioned `play_speed_present` or explicit blank predicate matches | Unique glyph match when present; a validated blank template yields `not_applicable` |
| graph type | Always | Unique match among `gauge`, `lanes`, and `measures`; no-match never defaults to `gauge` |
| options aggregate | Only when graph type is `gauge` | Unique option-block or explicit option-off template; its enum/boolean members form one known value |
| play type | Always | `SP` follows a known SP play mode. DP requires explicit battle/non-battle evidence from applicable options or an accepted same-episode selection; otherwise it is `unknown`, never default DP |
| graph target | When the versioned graph-target-region-present predicate matches | Unique closed-set target match; a validated absent-region template yields `not_applicable` |
| clear, DJ level, score, miss current values | Always | Every required slot must match uniquely and pass range/cross-field validation |
| clear, DJ level, score, miss best values | When that best-value slot has a validated present predicate | Unique value when present; a field-specific blank/no-record template yields `not_applicable` |
| clear, DJ level, score, miss `is_new` | When the corresponding current-value slot is present | Explicit NEW/not-NEW evidence; absence of the NEW match alone is not `false` |
| result tab | Always | Unique `rival` or `radar` tab anchor |
| rival rank before/now/position | Only when tab is `rival` | Every visible rank slot is unique; any UI-level no-rank state must be an explicit closed enum/template |
| radar attribute/chart value/value | Only when tab is `radar` | Unique attribute and complete numeric glyph matches |

## Music select

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| music-select state, play mode, has-score-data | Preconditions for every music-select event | Independent screen/layout anchors and unique closed-set matches |
| song, version, selected difficulty, selected level | Always | Unique resource match, OCR agreement where required, and title/version/play-mode/difficulty/level catalog consistency |
| play type | Always | Known play mode when score data is present. The no-score visual class may imply `DP BATTLE` only after its meaning is independently fixture-validated; otherwise `unknown` |
| per-difficulty levels | Only for charts present in the adopted catalog | Unique level glyph for every present chart; catalog-absent charts are `not_applicable`, not failed matches |
| clear, DJ level, score, miss | Only when `has_score_data == true` | Unique complete value matches and range validation; all are `not_applicable` when the independently recognized no-score state is true |

## Change control and tests

- Layout/resource adapters encode the named presence, absence, positive, and
  negative predicates. They may not turn matcher failure into applicability.
- Each row needs positive, legitimate-absence where applicable, ambiguous, and
  corrupt fixture cases. Conditional rows also need both predicate branches.
- Savable gating specifically needs savable, non-savable, unknown-background,
  overlay, and partial-corruption fixtures; only the first class may emit.
- Replay compares deterministic domain fields and issues. UUIDv7 and delivery
  wall time are transport metadata and are excluded.
- A new field or changed predicate is a schema change. Update this table, the
  typed event schema, and replay fixtures in the same logical change.
