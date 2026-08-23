# ADR 0032: Observe complete screen field sets without acceptance

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0030's field-observer worker and ADR 0031's registered text runtime

## Context

Live routing produces one complete measured crop set for either a result screen or a music-select
screen. The registered runtime can observe bounded text crops, but a title-only or artist-only
output would recreate an incomplete intermediate shape and could be mistaken for a screen or song
decision. The currently measured non-text fields also do not yet have registered observers.

Field inference runs asynchronously, so its result must remain joined to the worker-authored run,
sequence, timing, and screen provenance. Diagnostic recording must make inference success or failure
observable without storing OCR strings, pixels, catalog strings, or inventing acceptance policy.

## Decision

The v1 worker output is exactly one `ScreenFieldObservations` value:

- `ResultScreenFieldObservations` contains title and artist open-text observations plus explicit
  `observer_not_implemented` values for difficulty, level, notes, and current score;
- `MusicSelectScreenFieldObservations` contains central-title, artist, and active-list-title
  open-text observations plus an explicit `observer_not_implemented` value for selected chart.

There is no title-only, artist-only, supplemental-context, or optional-field screen output. The same
registered PP-OCRv6-small session observes each text crop in a fixed screen-local order on the
field-worker thread. Model, dictionary, catalog, or filesystem construction never occurs in
`observe`. If a text inference fails, the whole screen observation is an error carrying the exact
failed field and the underlying runtime cause; it is not a partial successful screen observation.

Open text, input width, and output timesteps remain imperfect observations. They grant no field
validity, confidence, catalog match, song identity, stabilization, suppression, accepted-result, or
event authority. `observer_not_implemented` means only that this runtime did not observe that crop;
it is not a guessed value or evidence that the field is absent.

The application may record one `observe_fields` diagnostic fact after receiving a bound worker
result. It first verifies the worker run ID and full binding identity against the active recognition
session. A success fact contains only screen plus fixed observed/unimplemented field counts. A
failure fact contains the screen and failed field with stable `field_observation_failed` type.
OCR strings, pixels, catalog strings, paths, environment strings, and arbitrary properties are not
diagnostic fields. The typed worker result is returned before and independently of diagnostic
enqueue; recording disabled, queue drop, or storage failure cannot change it.

## Consequences

- Result and music-select observations have complete screen-local shapes even while registered
  observers are missing for some fields.
- The field-worker output can be replayed or reduced later without inferring missing values from
  absent optional members.
- Catalog resolution, cross-field validation, temporal agreement, selection context, song decisions,
  event emission, real INFINITAS field accuracy, and target-host performance remain later gates.
