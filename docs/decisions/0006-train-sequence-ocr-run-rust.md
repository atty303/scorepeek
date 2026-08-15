# ADR 0006: Train sequence OCR offline and run catalog-constrained inference in Rust

- Status: Accepted
- Date: 2026-08-15
- Supersedes: ADR 0003

## Context

Upstream title templates require one artifact per song and inherit a different
rendering domain. A song-class model has the same new-song maintenance problem.
A general OCR string followed by fuzzy lookup can silently select the wrong
catalog title.

## Decision

Fine-tune a PP-OCRv6 small recognition model from private human-labelled game
crops and independently licensed/generated synthetic text. External catalog
strings are inference lexicon entries, not training examples.

Export the model to ONNX and run it in Rust. Score raw CTC logits directly
against exact catalog title variants, requiring an absolute bound, runner-up
margin, temporal agreement, and compatible chart context. Never expose a free
OCR string as an accepted domain value.

Python exists only in pinned offline training/export tooling. Runtime model
auto-download and Python fallback are prohibited. If PP-OCR export cannot pass
Python/Rust parity, replace the training architecture with an ONNX-exportable
CRNN/SVTR-style CTC model rather than weakening the runtime boundary.

## Consequences

- A catalog-only update can recognize a new song when its tokens and visual
  style are already covered.
- New glyphs and domain shifts remain unknown until the private corpus and model
  are updated.
- Model, dictionary, preprocessing, runtime, and catalog replay bindings must be
  versioned together.
