# Field semantics and applicability

This document is the v1 source of truth for deciding whether a recognition
field is `known`, `unknown(reason)`, or `not_applicable`. Layouts, predicates,
and recognizers are owned by scorepeek and calibrated from the private corpus.

Matcher failure never proves absence or `false`. `not_applicable` requires a
positive screen-state or validated absence predicate. If that predicate cannot
be established, the field is `unknown`.

## Shared evidence rules

- A screen state is `known` only when its independently calibrated anchors are
  unique and incompatible states, transitions, overlays, and negative scenes
  are rejected.
- A closed enum is `known` only when one class satisfies both its absolute
  acceptance bound and runner-up margin. No-match never selects a default.
- A number is `known` only when every visible slot is accepted and the complete
  value passes its domain and cross-field constraints. Partial digits do not
  form a value.
- A song is `known` only when full-catalog resolution has one accepted identity
  and all independent image context available on that screen is compatible.
  Result uses title, artist, play mode, difficulty, level, and notes; music
  select uses central title, artist, play mode, selected difficulty, selected
  level, and the active right-list title. The two music-select title
  presentations corroborate one selection and are not independent metadata
  votes; a readable conflict rejects. Version
  is additional evidence only when an independent version field is recognized.
  Candidate metadata cannot corroborate itself. Raw OCR text is not a value.
- A boolean needs calibrated positive and negative evidence. Failure to match
  the positive class is `unknown`, not `false`.
- All screen-local evidence used by one event must have the same capture generation,
  capture profile, normalizer, canonical layout, model/catalog binding, and
  temporal episode.

## Result

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| result state | Precondition for every result event | Unique result-screen anchors; transition, cut-in, overlay, and unknown classes reject |
| savable | Always for a result candidate; must be `known(true)` to emit | Unique positive or negative state; unknown background, animation, overlay, and corruption remain unknown |
| playside | Always | Unique 1P or 2P layout anchors |
| play mode | Always | Unique SP or DP evidence |
| play type | Always | SP follows known SP mode; DP battle/non-battle needs explicit evidence and never defaults |
| song | Always | Accepted title and artist consistent with independently recognized play mode, difficulty, level, and notes; a linked selection may corroborate identity but cannot establish result presence or result-only fields |
| difficulty and level | Always | Unique closed difficulty and complete level value consistent with the accepted catalog chart |
| notes | Always | Complete positive integer consistent with the recognized result layout |
| current score | Always | Complete non-negative value satisfying `score <= 2 * notes` |
| clear, DJ level, miss | Applicable after each field's presence predicate is independently calibrated | Unique complete value; `miss <= notes` when present |
| best/current/new, options, graph, play speed, dead/loveletter, rival/radar | Optional v1 capability only after its named layout and presence/absence predicates pass a dedicated release gate | Complete field-specific evidence; unsupported capability is `not_applicable`, enabled-but-unrecognized capability is `unknown` |

A result event requires all mandatory rows to be `known`, `savable == true`,
catalog consistency, and temporal stability. Optional fields may be
`not_applicable`; an optional field that is applicable but unknown does not get a
guessed value and may block full-capability advertisement without blocking the
minimal result event.

## Music select

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| music-select state | Precondition for every music-select event | Unique layout/state anchors; rapid scroll, transition, overlay, and unknown classes reject |
| play mode | Always | Unique SP or DP evidence |
| song | Always | Accepted central title and artist consistent with play mode, selected difficulty, selected level, and the active right-list title when readable |
| selected difficulty and level | Always | Unique selected state and complete level consistent with the accepted catalog chart |
| INFINITAS status | Catalog metadata, not an image field | `confirmed_present`, `unknown`, or `conflicted` from the active catalog snapshot; never inferred from source absence |
| has score data, clear, DJ level, score, miss, per-difficulty levels | Optional v1 capability after each presence/absence predicate is calibrated | Unique complete values; validated no-score state makes score fields `not_applicable` |

A general-IIDX title whose INFINITAS status is `unknown` may be accepted only by
the separately calibrated stricter title/context policy. The event preserves
`unknown`; recognition never upgrades catalog availability.

## Temporal and change control

- Stability uses distinct, fresh observations from one capture generation and
  the versioned minimum dwell. A disconnected or stalled source cannot turn one
  old frame into temporal evidence.
- Result emits once per result episode. Music select deduplicates a stable
  `(song, play mode, selected difficulty)` identity until it changes or the
  screen episode ends.
- A screen-local episode ends on screen exit. Separately, the last stable
  music-selection candidate set may contextualize result song resolution.
  Confirmed non-state scenes, unrecognized frames, gameplay, result, and retry
  preserve it; a new stable selection replaces it. Confident title/session end,
  a recording coverage gap, source reconnect, or any profile/normalizer/layout/
  catalog/model/runtime binding change clears it. Recognition failure alone is
  not a coverage gap. The context does not infer mode, attempts, or play count.
- Replay compares deterministic domain fields and issues. Transport event IDs
  and delivery wall time are excluded.
- Every field needs positive, legitimate-absence where applicable, ambiguous,
  corrupt, overlay, and negative fixture cases before it is advertised.
- Adding a field or changing applicability is an event-schema change and must
  update this document, typed schema, corpus labels, and replay gates together.
