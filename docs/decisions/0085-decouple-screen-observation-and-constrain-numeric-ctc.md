# ADR 0085: Decouple screen observation and constrain numeric CTC decode

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0059 for result gap continuity, ADR 0075 for field-worker busy handling, and ADR
  0083 for numeric field decode

## Context

The expanded result observer performs nineteen field decodes. A complete target run showed that the
bounded single field worker commonly remained occupied for roughly 500 ms. ADR 0075 treated every
due tick during that interval as a recognition busy skip, so the application also failed to observe
screen and session boundaries. ADR 0059 then used the sparse field-completion timestamps as an
episode-continuity proxy and reset otherwise equal result evidence after 250 ms.

The same run preserved `只` as unrestricted OCR for the visually numeric BAD zero. A BAD-only glyph
substitution and a larger temporal gap would repair that sample, but both encode properties of one
run instead of the actual contracts: screen continuity is visible at 10 Hz independently of field
OCR, and numeric fields have a smaller valid alphabet than the model dictionary.

PP-OCRv6-small already produces logits for every registered dictionary class. Restricting the
greedy candidate set after inference can use those same logits without changing the model,
dictionary, worker count, or number of inference calls.

## Decision

- The fixed 100 ms recognition cadence always evaluates the screen predicate. It advances
  `screen_changed`, play-attempt state, and predicate diagnostics even while one field observation
  is outstanding.
- Busy suppresses only result/music-select crop routing and field-worker submission. Record this as
  `field_observation_busy_skip`, separately from queue rejection or the legacy
  `recognition_busy_skip`, with total and maximum-consecutive counts in the bounded diagnostic.
- Result song and clear type stabilize after two equal observations in the same explicitly observed
  result episode. Remove the result observation gap. Screen change, session boundary, unknown,
  conflict, and reversed monotonic time retain fail-closed behavior. Music-select keeps its separate
  250 ms maximum-gap policy.
- One PP-OCRv6 inference produces unrestricted greedy text and, where configured, a constrained
  greedy decode from the same logits. Both decoders retain CTC blank and repeat-collapse semantics
  and deterministic lower-token tie ordering.
- Level, notes, current score, and all five judgments allow only ASCII digits. Previous score,
  previous miss, current miss, FAST, SLOW, and combo break additionally allow the already-supported
  dash characters. All other fields use unrestricted decode.
- `DynamicTextObservation` keeps unrestricted `open_text` and optional `constrained_text`
  separately. Numeric typed parsers consume the constrained value; raw text is never used as a
  numeric fallback in production and remains diagnostic evidence. Do not add a BAD- or `只`-local
  substitution. Preserve the independently established current-score cyan retry and its bounded
  trailing-eight resolution, but keep its unrestricted retry text unchanged.
- Advance result field resolution to
  `scorepeek-result-fields-catalog-constrained-v2`, recognition observations to v8, and complete
  joined diagnostics to v4. V8 field records contain raw, constrained when applicable, and typed
  resolution. Existing recognition v5-v7 and diagnostic v2/v3 artifacts remain readable and are
  never rewritten.
- Recording opt-out, queue pressure, or artifact failure does not change screen, field, temporal,
  attempt, or domain-event semantics.

## Consequences

- Field throughput remains bounded and latest-only, but expensive OCR no longer hides a result exit
  or another session path transition.
- Result continuity is owned by observed screen boundaries rather than an uncalibrated OCR duration.
- Numeric errors can be corrected by model probability within the field vocabulary while retaining
  the unrestricted model output for investigation.
- Target authority remains unproven until a newly installed binary demonstrates approximately 10
  screen ticks per second, field busy skips without recognition skips, accepted performance and
  confirmed attempt state, exactly one v2 result domain event, and zero recording drops.
