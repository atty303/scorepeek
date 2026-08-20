# ADR 0020: Select an official ONNX recognizer before custom training

- Status: Accepted
- Date: 2026-08-20
- Supersedes: ADR 0006 for its mandatory PP-OCRv6-small fine-tuning and custom-export sequence;
  ADR 0018 only for model candidates requiring set-inclusion growth
- Complements: ADR 0018's evidence sequence and ADR 0019

## Context

ADR 0006 selected PP-OCRv6-small fine-tuning before scorepeek had measured the available official
deployment models against its actual closed catalog. That sequence led to a scorepeek-specific
dictionary, mapped training initializer, fine-tuning pilots, and a custom Paddle-to-ONNX export path.
The later complete stationary corpus showed that song identity, not open-text transcription, is the
selection objective. Applying the accepted comparison key in the decoder recovered two incomplete
songs without changing model weights and invalidated the earlier exact-only pilot comparison.

PaddlePaddle now distributes immutable ONNX recognition artifacts that run directly in ONNX Runtime.
The registered PP-OCRv6-small official graph has already passed Paddle/Rust tensor and candidate-rank
parity. It was not measured on the complete corpus because the bootstrap decoder rejected the whole
catalog when any display variant was unencodable. Requiring every variant to be encodable is stronger
than the product requirement that every song remain safely distinguishable.

Continuing from the mapped initializer would make scorepeek own training, dictionary mapping, export,
and parity costs before establishing that an official runtime artifact is insufficient. Other official
recognition models may also have a better accuracy, size, or latency tradeoff than PP-OCRv6-small.

## Decision

- Evaluate official, immutable, license-compatible ONNX text-recognition artifacts before selecting or
  training a scorepeek-owned model. The initial bounded comparison includes PP-OCRv6 tiny, small, and
  medium plus PP-OCRv5 mobile and server when an official ONNX artifact and its complete preprocessing
  and dictionary metadata can be pinned.
- Preserve each model's official input geometry, preprocessing, token dictionary, timestep count, and
  output contract. “Same basis” means the same crops, catalog, comparison-key contract, song-level
  decision semantics, and metrics; it does not mean reshaping distinct models into one tensor contract.
- Phase one compares unmodified official models with one common decoder policy. Phase two applies each
  global, model-free decoder candidate to every registered official model whose immutable contract can
  run and whose song-domain safety audit can be satisfied. Phase-one coverage or rank cannot remove a
  model from phase two. Record an explicit reason for every model excluded because its contract cannot
  run or its safety gate cannot be satisfied. Do not tune a decoder exception for one model or song and
  then compare it with untuned alternatives.
- The primary selection metric is the number of the 1,119 stationary-corpus songs whose every eligible
  crop resolves to the correct unique catalog song. Report gained and lost song sets for every comparison,
  but do not require the new set to contain the old set: that monotonic constraint can reject a model with
  better global coverage. Wrong unique decisions remain failures and later live acceptance still requires
  calibrated zero-false-accept gates. Open-text accuracy remains diagnostic. Title-disjoint validation
  and evaluation remain generalization guards rather than the primary selection oracle: a candidate
  cannot make an already fully recognized held-out song incomplete. If correct-song coverage is equal,
  prefer fewer wrong unique crop decisions, then the official artifact with the smaller and simpler
  runtime bundle, then measured target latency.
- Replace the old every-variant coverage gate with a song-domain safety gate. Every active song must
  retain at least one encodable sequence or a separately accepted collision-safe model-free signature.
  A song with no representation remains unknown; its removal must not make another song acceptable by
  silently shrinking the competitive domain.
- Keep the mapped initializer only as a diagnostic comparison point. Do not export it for runtime,
  re-evaluate historical fine-tuning pilots, or start new training until the official-model matrix and
  uniform decoder alternatives have been measured.
- Consider custom dictionary mapping, training, and export only if the official candidates remain
  materially below the finite-corpus song-identity goal after global decoder and stationary multi-frame
  alternatives. The measured residual failures must justify the additional owned pipeline.

## Consequences

- The existing official PP-OCRv6-small ONNX graph becomes the first complete-corpus candidate, not the
  mapped initializer.
- Model comparison requires a model-independent census interface and per-model registered metadata, but
  no training framework or export step.
- Unsupported characters and overlong variants are decoder-domain evidence rather than an automatic
  catalog-wide failure. The runtime still fails closed when song competition cannot be represented.
- Existing mapped initializer, pilot, and export artifacts remain reproducible historical diagnostics;
  they are not active runtime candidates or model-selection evidence.
- Result-centered release acceptance and stationary music-list evidence roles from ADR 0018 are unchanged.
