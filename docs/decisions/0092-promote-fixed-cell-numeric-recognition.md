# ADR 0092: Promote fixed-cell numeric recognition

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0087 for production numeric-model authority and level veto;
  promotes ADR 0090 and ADR 0091 fixed cells to production authority

## Context

The full-field numeric CTC recognizer failed reviewed PGREAT, POOR, and combo-break crops even
though every displayed digit occupies a fixed canonical position. The character-cell spike
classified the reviewed glyphs once dynamic component discovery was removed. Score, judgments,
timing, combo break, and level use different colors and geometries, but field-family hard and soft
masks can normalize those differences before one shared classifier. A separate level model is not
justified by current data, and level is not needed to validate a chart after song identity is
confirmed.

## Decision

- Canonical numeric character layout v2 is the only production segmentation authority. Runtime
  crops its declared cells directly; it never detects components, moves slots, or infers gaps from
  pixels.
- Every cell is converted to a 2,244-value hybrid feature: field-family hard and soft masks,
  `24x32` linear resize, nine-direction coarse `8x8` and fine `4x4` HOG, normalized soft pixels,
  fine coefficient `0.25`, and final L2 normalization. Rust owns the game-session implementation;
  the offline Python reference and immutable build report establish parity.
- One dynamic batch submits all visible cells to a private ONNX MLP with shape
  `N x 2244 -> N x 11`, classes `_0123456789`, and hidden width 64. Fixed-slot grammar permits
  leading blanks only, followed by contiguous decimal digits. Notes preserves four displayed
  digits, level is bounded to 1--12, combo break to three slots, and other numeric fields to four.
- Dash is not a classifier class. Previous score, previous miss, and current miss use a separate
  fixed marker ROI and a bounded horizontal-stroke predicate. Only two equal observations can
  promote the marker to `not_displayed`; uncertain evidence remains `unknown`.
- The score, PGREAT, and GREAT candidates retain `score = 2 * pgreat + great`. Scores are bounded
  by twice notes and PGREAT, GREAT, GOOD, and BAD individually by notes. No judgment-total rule is
  imposed. POOR, miss values, FAST, SLOW, and combo break are not bounded by notes.
- Level is independently calibrated so a wrong top class becomes `unknown`. Before song identity
  is accepted, high-confidence level may only narrow candidates already established by title,
  artist, difficulty, and notes. It never creates a song candidate. After song identity is
  accepted, chart resolution ignores observed level, records any catalog mismatch as debug
  evidence, and uses the accepted catalog chart level. A level mismatch cannot suppress
  performance, attempt confirmation, or a domain event.
- The current private model manifest is v2 and binds the character layout, feature contract,
  dataset/evaluation/final-training generations, tensor shape, calibration, and ONNX digest.
  Legacy CTC manifests remain readable but cannot be activated as current authority. Missing,
  changed, or tensor-incompatible fixed-cell resources fail startup closed without PP-OCR fallback.
- Recognition artifact v10 records cell and field candidates, model/preprocessor identity, joint
  and temporal decisions, level/catalog mismatch, and event suppression. Accepted
  `scorepeek-result-detected-v2` and the domain-primary TUI remain unchanged; classifier evidence
  is debug-only.

## Consequences

- General PP-OCR remains responsible for result text and music-select text, while one small
  fixed-cell batch is the sole numeric authority.
- Optional numeric fields may remain unknown but may not become wrong known values. Mandatory
  score and judgments still require two equal observations in one result episode before joining an
  accepted play attempt, in either arrival order, into one domain event.
- Private weights, real crops, complete labels, and generated datasets remain outside the
  repository. Source code, layout, registered manifest, schemas, and reproducible evaluation
  tooling remain reviewable.
- Target support and public socket authority remain unclaimed until a separately installed build
  passes the prospective ten-episode, two-session gate with zero wrong accepts, one event per
  attempt, and zero drops.

## Development evidence

- Corrected suite generation
  `647b544669e190e3ac484c53eeed6bb5c72d1d83c6bef054cbb6048ca2716bb6`
  contains seven sessions, twenty-seven episodes, 2,598 canonical frames, and one negative frame.
  The production fixed-cell runtime replays the complete suite successfully.
- Session-disjoint evaluation classifies 339 of 340 stable numeric field observations exactly.
  The sole wrong top prediction is level `10` as `11`; the independently selected level threshold
  turns it into `unknown`. No non-level stable field has a wrong top prediction.
- The fixed dash predicate classifies all 167 reviewed marker-capable source rows correctly,
  including both dash and numeric displays.
- Regression sentinels include `Horizons of Promise`, PGREAT `587`, POOR `3`, and combo break `30`.
  Runtime replay accepts every required judgment tuple and never promotes a wrong optional known
  value.
