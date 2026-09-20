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
- A song is `known` only when full-catalog resolution has one accepted identity.
  An already accepted primary song remains known when auxiliary chart evidence is missing or
  contradictory; the chart and complete event become unknown instead of downgrading the song.
  When the primary result is still ambiguous only at its margin or artist-corroboration gate, a
  catalog-unique chart may narrow it to the selected primary candidate.
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

PLAY presence scans the single measured BPM-outline area `x=260..1659, y=940..1079` without
dividing it by side. A candidate requires a 280--305 pixel cyan top edge, a 300--320 pixel cyan
bottom edge 59--70 rows below it, and at most one pixel of horizontal center displacement. Adjacent
row observations of the same center are one candidate; exactly one distinct candidate is required.
Only observations within the measured 12-row edge-slope window are merged as one candidate; a
second vertical outline at the same horizontal center remains distinct. Interior pixels, including
DETAIL JUDGE overlap, do not participate, and BPM position never implies play side.

## Result

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| result state | Precondition for every result event | Unique result-screen anchors; transition, cut-in, overlay, and unknown classes reject |
| accepted play attempt | Precondition for result-event emission | The same attempt observed gameplay and result, and its result song confirms the selected or retry-inherited song; observing the decision transition is not required |
| play side | SP only | RESULT presence first requires exactly one panel anchor: left or right. Two fresh observations of the same panel side stabilize the episode. SP maps left to `known(one_player)` and right to `known(two_player)`; DP emits `not_applicable`. BPM position is not play-side evidence. |
| play mode | Always | Derived from the final catalog play type as `single_play` or `double_play`; raw RESULT mode is independent evidence requiring exact `SP`/`DP` twice and no opposite observation in the episode |
| play type | Always | The highest-consistency catalog play type after independent equal-weight SELECT and RESULT mode families; a conflict never removes either type, and insufficient chart margin remains unknown |
| song | Always | Accepted title and artist consistent with independently recognized play mode, difficulty, level, and notes; a linked selection may corroborate identity but cannot establish result presence or result-only fields |
| difficulty and level | Always | Unique closed difficulty plus a catalog-unique chart; unreadable level may come from that chart, while a readable conflict rejects the chart |
| notes | Always | Complete positive integer consistent with the recognized result layout |
| current score | Always | Complete non-negative value satisfying `score <= 2 * notes` |
| judgments | Always | All five of `pgreat`, `great`, `good`, `bad`, and `poor` are complete, each is at most notes, and `current score == 2 * pgreat + great`; their sum is not constrained to notes |
| clear | Always for the admitted result layout | One registered clear-type value from the exact `CLEAR TYPE` field, never the result background |
| miss count, fast, slow, combo break | Supplemental result values | Complete non-negative value at most notes, a displayed dash as `not_displayed`, or an explicit `unknown(reason)`; unknown does not block the event |
| previous best clear/score/miss | Reference snapshot | Each field is independently `known`, `not_displayed`, or `unknown(reason)`; recognized `NO PLAY` normalizes all three to `not_played`; score is at most `2 * notes` and miss is at most notes |
| DJ level, score delta, NEW RECORD, percentage | Derived or excluded | Do not save from OCR; derive from score, notes, and previous score when needed |
| play options | Supplemental result value | The complete fixed panel ROI is parsed against the finite ordered vocabulary; two matching typed observations are required. Conflict or incomplete evidence is `unknown` and does not block an otherwise accepted result. |
| graph, play speed, dead/loveletter, rival/radar | Not represented | No placeholder or inferred value is emitted. |

A provisional result lifecycle value requires all mandatory result fields to be `known`, two
matching numeric observations, joint catalog consistency, and an active RESULT attempt ID. It does
not require selection linkage, observed gameplay, or final attempt confirmation. A confirmed
`result_changed` requires that same stable payload to remain accepted at semantic RESULT close;
missing gameplay or selection linkage does not invalidate an otherwise complete attempt, while song
conflict or an abandoned attempt suppresses confirmation. Supplemental and previous-best unknowns do
not get guessed values and do not block the shared result payload.

The common RESULT header and title/artist/chart ROIs are screen-global. Clear, score, previous-best,
judgment, FAST/SLOW, combo-break, and play-option ROIs are panel-local with origins `x=0` and
`x=1360`. The screen predicate supplies `ResultPanelSide` to the crop router; cropping never
re-detects the side. Only field observations whose side equals the stabilized episode side enter
semantic accumulation. One opposite observation is discarded, unknown observations preserve the
stable side, and two opposite observations retract a provisional result as an episode conflict.

## Music select

| Field | Applicability predicate | Evidence required for `known` |
| --- | --- | --- |
| music-select state | Precondition for every music-select event | Unique layout/state anchors; rapid scroll, transition, overlay, and unknown classes reject |
| play side | SP only | For SP, exactly one fixed footer label, `PLAYER 01 SIDE` on the left or `PLAYER 02 SIDE` on the right, exceeds the calibrated bright-pixel minimum and winner margin; the same side must occur twice with no opposite observation in the episode. DP emits `not_applicable` and does not require either footer label. |
| play mode | Always | The fixed SELECT badge matches exactly one registered SP/DP template above minimum score and winner margin; the same type must occur twice with no opposite observation in the episode |
| song | Always | Accepted central title and artist consistent with play mode, selected difficulty, selected level, and the active right-list title when readable |
| selected difficulty and level | Always | Unique selected state and complete level consistent with the accepted catalog chart |
| INFINITAS status | Catalog metadata, not an image field | `confirmed_present`, `unknown`, or `conflicted` from the active catalog snapshot; never inferred from source absence |
| score and miss count | Supplemental SELECT-best fields | Each value stabilizes independently after two equal fresh observations. An unreadable value is `unknown`; the measured dash pattern is explicit no-record. |
| clear type | Supplemental SELECT-best field | One registered clear value after two equal fresh observations; explicit no-record clears the stored supplement. |
| DJ level | Derived presentation | Calculated from EX SCORE and catalog notes; never OCR input. |

`music_selection_changed` is a UI-only lifecycle. `Selected` requires a unique catalog song/chart
under title, artist, selected difficulty, stable SELECT play type, and for SP a stable SELECT play
side. DP does not wait for footer play-side evidence. The
resolver emits no initial unknown, deduplicates equal states, emits unresolved after losing a
previously selected chart, and emits episode-ended unresolved at SELECT finalization. This state
cannot satisfy RESULT presence, attempt linkage, numeric stability, or `result_changed` acceptance;
RESULT play side remains on its separately admitted contract above. If stable SELECT and SP RESULT
play sides disagree, the complete RESULT remains authoritative; only the attempt linkage and the
retained SELECT context are discarded. The mismatch is recorded diagnostically, and that SELECT
context is not reused until a new stable selection replaces it.

A general-IIDX title whose INFINITAS status is `unknown` may be accepted only by
the separately calibrated stricter title/context policy. The event preserves
`unknown`; recognition never upgrades catalog availability.

## Temporal and change control

- Stability uses distinct, fresh observations from one capture generation and
  the versioned minimum dwell. A disconnected or stalled source cannot turn one
  old frame into temporal evidence.
- RESULT panel-side state is cleared by positive screen exit, capture-session end, or a new
  immutable binding. Unknown predicate frames never count as opposite-side evidence and do not age
  out a stable side.
- `result_changed` emits an envelope event ID and sequence for provisional, retracted, re-resolved,
  and confirmed transitions; there is no result-local revision. Confirmed emits once per attempt.
  Music select deduplicates a stable
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
- Every represented field needs positive, legitimate-absence where applicable,
  ambiguous, corrupt, overlay, and negative fixture cases.
- Adding a field or changing applicability is an event-schema change and must
  update this document, typed schema, corpus labels, and replay gates together.
