# ADR 0086: Score finite numeric CTC sequences exactly

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0085 for constrained numeric decode and result ROI measurements

## Context

ADR 0085 restricted each timestep's greedy candidate set to blank plus a field-local alphabet. That
corrected one retained BAD crop after its ROI was moved to the measured row, but the decoder still
made a local choice before CTC collapse. A sequence can have greater total probability across all
valid CTC alignments even when no individual timestep chooses it greedily. Numeric judgments are
event-authoritative inputs, so correctness is worth a small exact dynamic program rather than a
sample-specific substitution or an uncalibrated beam width.

The existing catalog-title decoder already used the required exact CTC prefix recurrence. The
numeric language is much smaller than the song catalog and is fixed independently of any captured
image. No compatible Rust crate improves this bounded case without importing a larger decoder
framework or introducing approximate pruning.

Operator review of result crops also found that the earlier clear type, level, judgment, and combo
break regions did not contain their complete future value widths and that the judgment crops
contained unnecessary vertical background.

## Decision

- Share one generic CTC sequence trie between catalog-title scoring and numeric fields. For every
  permitted output string, sum the probabilities of all alignments that CTC-collapse to that exact
  string, including blank-separated repeated digits.
- The digit language contains every one- through four-digit display spelling, including observed
  fixed-width leading zeroes such as `0764`. Level is bounded to two display digits and combo break
  to three; other numeric fields retain four. Dash-enabled languages additionally contain every
  one- and two-character sequence made from the registered `-`, `―`, `ー`, and `—` dictionary
  tokens. Field parsers retain their narrower semantic bounds from notes and score context.
- Compare the best permitted non-empty sequence with the exact all-blank path. Emit constrained
  empty text when blank wins or ties; otherwise emit the highest-probability sequence. Equal
  non-empty candidates use lexical order only as a deterministic tie break. Add no confidence
  margin, glyph substitution, beam width, pruning, extra inference, model change, or dependency.
- Keep unrestricted greedy OCR unchanged as raw evidence. Recognition artifact v8 continues to
  store raw, constrained, and typed values separately. The runtime manifest advances its decoder
  identity in immutable `pp-ocrv6-small-live-runtime-v2.json` to
  `scorepeek-ctc-open-greedy-numeric-exact-v1`, while the registered v1 manifest remains unchanged.
  Result field resolution advances to `scorepeek-result-fields-catalog-constrained-v3`.
- Adopt the reviewed result ROIs: clear type `(360,400,154,55)`, level `(845,1032,65,32)`, each
  judgment `(370,y,150,30)` for `y=778,808,838,868,898`, and combo break
  `(440,970,110,50)`. These regions cover the complete clear-type right edge, two-digit level,
  four-digit judgments, and three-digit combo break while reducing judgment background height.

## Consequences

- Numeric decode now maximizes sequence probability under an explicit finite display language instead of
  depending on locally greedy choices. Synthetic exhaustive tests cover blank competition, shared
  prefixes, repeated digits, dash repetition, and deterministic ties.
- The scorer adds bounded CPU work after each existing ONNX inference. Its largest numeric trie has
  about eleven thousand prefixes over forty timesteps and does not allocate an unbounded beam.
- Raw OCR remains available when the constrained decision is surprising, while typed range checks,
  two-observation temporal stability, and the score-breakdown invariant remain the event authority.
- Existing immutable recognition artifacts remain readable. Their recorded runtime and layout
  digests continue to identify the older decoder and crops rather than being reinterpreted.
