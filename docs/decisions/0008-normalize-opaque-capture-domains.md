# ADR 0008: Normalize opaque capture domains before shared recognition

- Status: Accepted
- Date: 2026-08-16

## Context

The game has a conceptual image before Wine, Vulkan, Gamescope, compositor,
capture, and transport effects, but scorepeek cannot reliably observe a native
pixel reference for it. Portal, Gamescope direct PipeWire, OBS, and future
capture routes can be stable enough to support recognition without being
pixel-identical or decomposable into independently invertible layers.

Making one observable route the correctness reference would couple recognition
to an incidental capture stage. Conversely, allowing each route to train an
independent recognizer would duplicate semantic behavior and make later tuning
hard to validate across environments.

## Decision

Capture adapters produce an `ObservedFrame` in one explicit, immutable capture
profile. A versioned domain normalizer maps that opaque profile to a conceptual
RGB8 1920x1080 `CanonicalFrame`; the profile and normalizer do not model Wine,
Vulkan, Gamescope, compositor, PipeWire, or other internal layers separately.
There is no required capturable pixel target for the canonical representation.

Normalizers use deterministic geometry, color, and filtering first. A learned
residual adapter is permitted only when measured recognition evidence requires
it, must be deterministic and bounded, and must not be a generative text or
image restorer. Correctness is established from human labels, cross-field
invariants, negative scenes, and deterministic semantic replay, not pixel
equality with another backend.

The canonical frame preserves color for all field recognizers. OCR-specific
grayscale, contrast, resize, padding, and tensor normalization happen after ROI
extraction in a versioned OCR preprocessor bound to the OCR model. Training and
Rust inference must use the same preprocessing contract.

A supported model bundle binds the canonical contract, capture profiles,
normalizer artifacts, layout profiles, OCR preprocessors and models,
profile-specific thresholds, corpus generations, and runtime compatibility.
The runtime never switches profiles or normalizers silently. An unknown or
drifted profile fails closed.

Initial offline training may use any available dedicated capture environment.
Later human-labelled captures from the normal Gamescope environment may extend
the corpus and produce a new immutable bundle. Promotion requires frozen
in-profile and cross-profile replay gates for every supported profile; runtime
self-labelling, online training, and automatic threshold relaxation are
prohibited. A stable normalizer may be reused across software or environment
changes while the observed contract and all gates continue to pass.

Portal, Gamescope direct PipeWire, and eligible OBS routes are peer capture
candidates. Each must independently pass semantic recognition, lifecycle, and
performance gates before it is supported; none is a pixel correctness oracle.

## Consequences

- A native or internal game image remains conceptual and is not a required
  fixture, calibration source, or release gate.
- Capture-pipeline implementation details may be retained as diagnostic
  provenance, but they do not shape the runtime normalizer interface.
- Capture-domain adaptation and shared recognition can evolve independently,
  while every promoted bundle is evaluated across the complete supported set.
- Corpus generations bind each source to its opaque capture profile,
  normalizer artifact, and layout profile. Replay suites select in-profile or
  profile-disjoint evaluation explicitly.
