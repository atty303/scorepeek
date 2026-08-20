# ADR 0019: Apply comparison keys to catalog-constrained CTC candidates

- Status: Accepted
- Date: 2026-08-20
- Supersedes: ADR 0006 only for exact-only catalog candidate sequences

## Context

ADR 0006 requires direct CTC scoring against catalog titles instead of accepting an open OCR
string. The later title identity contract
`scorepeek-title-nfc-ucd17-exact-then-ascii-width-fold-v2` treats an ASCII observation such as
`PASTELISM` as the same song candidate as the catalog display title `ＰＡＳＴＥＬＩＳＭ` when that
bounded fold is unique by song.

Provisional-label association implemented that comparison key, but the Python coverage census and
Rust catalog-title decoder continued to tokenize only raw catalog variants. Consequently the
source model could read and uniquely associate ASCII `PASTELISM` while every retained census
checkpoint was scored only against the fullwidth token sequence and selected another song. Model
coverage was therefore being measured against a different song-identity contract from provisional
association.

## Decision

- Derive CTC candidate sequences from every non-search catalog title variant using the registered
  comparison-key ID. Retain the raw variant and its exact comparison key.
- Add a bounded ASCII/fullwidth folded key as a decode alias only when that folded key maps to
  exactly one song across the complete candidate domain. A cross-song folded collision creates no
  alias. Identical exact sequences may remain attached to multiple songs and therefore cannot
  produce a unique decision.
- Score all retained sequences through the same CTC trie and use the maximum sequence score as the
  song score. Existing absolute evidence, runner-up margin, temporal agreement, and chart-context
  requirements remain unchanged.
- Bind the comparison-key ID to the catalog candidate artifact used for evaluation. Python census
  and Rust catalog-title decoding must derive the same candidate sequences and preserve score and
  ranking parity.
- Do not train a model to compensate for a missing comparison-key alias. Remeasure the complete
  finite corpus after candidate-domain changes before selecting any new training input.

## Consequences

- Width and ASCII-space presentation differences accepted by the identity contract can be resolved
  from existing CTC output without changing model weights.
- The decoder remains closed-set and collision-safe. It does not add case folding, Greek/Latin
  confusable folding, broad compatibility normalization, fuzzy edit distance, search terms, or
  song-specific exceptions.
- Catalog changes can add or remove a safe folded alias, so stored logits must be rescored against
  the complete current candidate domain before activation.
- The Rust decoder remains an offline diagnostic and future runtime path until the separate runtime
  integration and acceptance gates are implemented.
