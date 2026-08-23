# ADR 0036: Replay recordings through the production field path

- Status: Accepted
- Date: 2026-08-23
- Supersedes: ADR 0032 only for the complete result-screen field shape
- Complements: ADR 0010, ADR 0025, ADR 0026, ADR 0033, ADR 0034, and ADR 0035

## Context

The private corpus retains reusable recordings and digest-bound canonical extractions, while the
bounded field-observation gate previously accepted only a live Gamescope lease. Testing a recorded
play session through a separate crop or OCR command would not establish that the same screen
routing, worker ownership, registered runtime, and full-catalog scoring path behaves correctly.

Result backgrounds are not a finite red/blue class. Successful results in particular can use many
full-frame illustrations, so background color cannot determine result presence or clear outcome.
The exact observed outcome is rendered in the result screen's `CLEAR TYPE` field.

## Decision

Capture and offline extraction remain source-owned, and both sources stop at one common
`BoundCanonicalFrame` boundary. A Gamescope source acquires and normalizes an admitted frame. A
recording source reads the already verified canonical extraction derived from its corpus recording.
From that owner onward both sources use the same synchronous screen predicate, crop router,
application session, single registered field-observer worker, PP-OCRv6-small runtime, and
full-catalog candidate domain. The recording path does not construct an alternate OCR or scoring
pipeline and does not make the runtime decode MKV bytes.

A create-only `scorepeek-recording-field-simulation-profile-v1` canonical JSON artifact binds:

- recording bytes and digest, recording-manifest digest, its source-manifest digest, media-probe
  digest, capture-profile digest, and operator-reviewed coverage-label digest;
- canonical-extraction, normalizer, current canonical-layout digests, the complete extraction time
  span and frame count;
- catalog, model, and runtime digests;
- bounded source delivery pacing and diagnostic frame-sampling cadence; and
- ordered, non-overlapping result windows with their reviewed label timestamps and exact expected
  `CLEAR TYPE` text.

Authoring verifies the recording manifest, coverage label, extraction manifest, every selected
canonical frame, binding invariants, and that each expected result window contains a result frame.
The extraction's source-manifest digest must match the source digest declared by the selected
recording manifest. The profile episodes must exactly and uniquely cover every strictly parsed
result observation in the coverage label. Authoring also rejects any result frame outside the
declared windows. Publication is mode-0600, create-only, file- and parent-synced. Real profiles,
recordings, frames, labels, and diagnostic runs remain outside the repository.

Result presence requires the fixed `EXTRA STAGE RESULT` header together with two measured
horizontal result-panel boundaries. It does not inspect the full-frame background palette. The
complete result worker output now observes title, artist, and `CLEAR TYPE`; difficulty, level,
notes, and current score remain explicit `observer_not_implemented` values. A simulation episode
matches only after the expected exact `CLEAR TYPE` text is observed on at least two frames. Initial
animation frames and other nonmatching text never infer an outcome.

The simulation feeds every extraction frame through the common path, rejects result detections
outside the expected windows, requires a nonempty full-catalog candidate set for every submitted
screen, and requires every declared result episode to match. Public reports contain only digests,
counts, typed status, worker status, and diagnostic completeness. OCR strings and pixels are not
diagnostic facts. Source pacing and diagnostic sampling are profile-bound so an accelerated replay
does not manufacture queue degradation that is absent at the recorded cadence.

## Consequences

- Offline evidence can gate later live work without creating a second canonical recognition path.
- Failed versus successful result evidence comes from exact `CLEAR TYPE` observations, not result
  art or background color.
- The observed private session establishes only its two `FAILED` and one `CLEAR` episodes. It does
  not release other clear types, background variants, accepted song/result events, or target-host
  performance.
- A complete recording simulation is a prerequisite for a separately authorized live INFINITAS
  Gamescope run; it does not itself start or authorize that run.
