# ADR 0021: Search the full song catalog from imperfect text observations

- Status: Accepted
- Date: 2026-08-20
- Supersedes: ADR 0020 only for requiring every song to be directly encodable or replaced by a
  collision-safe model-specific signature before model evaluation
- Complements: ADR 0019 and ADR 0020

## Context

The first PP-OCRv6-small dictionary audit found 13 of 1,879 active songs with no exact sequence
representable by its 18,710-class dictionary and 40 CTC timesteps. None of those 13 songs occurs in
the current 1,119-song stationary corpus. Treating this audit as a model-evaluation gate would either
reject the official model before measuring it or derive shortened catalog signatures by removing
unsupported characters and truncating titles. Both choices confuse a recognizer's output contract
with song identity and can distort the competitive catalog domain.

The model does not need to transcribe every catalog title exactly to provide useful evidence. Its
open-text output can instead be treated as an imperfect observation and compared with every full
catalog title. This retains unsupported and overlong songs as competitors without pretending that a
shortened string is their identity.

A complete provisional census ran the immutable official PP-OCRv6-small ONNX graph once over all
3,061 stationary crops and applied three global search policies to the same observations. Exact
comparison-key search fully recognized 991 of 1,119 songs. Absolute Levenshtein distance raised this
to 1,108 songs with four wrong unique crop decisions. Normalized Levenshtein similarity reached
1,110 songs with three wrong unique crop decisions. It searched all 1,879 catalog songs; no title was
removed for dictionary or timestep coverage. Correct and incorrect normalized margins overlapped, so
the positive-only corpus does not calibrate a live acceptance threshold.

## Decision

- Keep every active song and its complete catalog title variants in the competitive domain during
  official-model evaluation. Dictionary and timestep audits describe model limitations; they do not
  remove songs, rewrite titles, or block an otherwise runnable census.
- Separate OCR observation from song search. Run each immutable official model with its native
  preprocessing, dictionary, and tensor contract, then apply the same global catalog-search policies
  to its output.
- Use exact comparison-key search as the unadjusted open-text baseline. Evaluate global edit-distance
  policies on the same model observations. A unique nearest song is still wrong when it differs from
  the label; ties remain unknown and are never guessed.
- Report fully correct songs, correct crop decisions, wrong unique crop decisions, unknown or tied
  decisions, and gained/lost song sets. Primary ranking remains complete-corpus fully correct songs,
  with wrong unique decisions as the first tie-break and a release-critical failure class.
- Do not promote a positive-only distance margin to a live threshold. Candidate acceptance still
  requires independent negative evidence and the result-centered gates in ADR 0018.
- Preserve the full text for future CTC-logit, partial-observation, multi-window, and stationary
  multi-frame decoders. Such policies must remain global and be compared across every runnable
  official model; they must not introduce per-song title rewrites or exceptions.

## Consequences

- Official models can be compared before solving every model-vocabulary mismatch.
- The current normalized-distance result is an evaluation lead, not a selected runtime decoder: its
  three wrong unique crops and overlapping margins require further global decoder work.
- The 13 songs absent from the stationary corpus remain search competitors, but their own recognition
  is unmeasured until stationary music-select or passive result evidence becomes available.
- ADR 0020's official-model-first sequence remains in force. Custom training or dictionary expansion
  is not justified by the static coverage audit.
