# ADR 0087: Specialize fixed-result numeric recognition

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0086 for numeric-model authority and decode, ADR 0083 for
  notes-based supplemental ranges, and ADR 0022 only for numeric result fields

## Context

The selected PP-OCRv6-small text recognizer remains useful for titles, artists,
clear types, and difficulty. It repeatedly failed to provide a stable numeric
tuple for fixed result-screen crops, however. Restricting its large dictionary
after inference improved individual observations but did not establish that a
future digit crop would remain in the correct CTC sequence. A result could
therefore be catalog- and attempt-confirmed while the required performance
tuple never acquired domain-event authority.

The numeric display has a much smaller domain than general text: fourteen fixed
ROIs contain only `0` through `9` and display dashes, with at most four glyphs.
The private corpus now has operator-reviewed v3 truth for every active result
episode. POOR and miss-like values can exceed chart notes because they include
events outside the note-judgement total, so a shared notes upper bound is not a
valid confidence rule.

## Decision

- PP-OCRv6-small remains the sole text authority for title, artist, clear type,
  difficulty, previous clear type, and music-select fields. It is not a
  production fallback or corroborating vote for any numeric field.
- A scorepeek-owned CTC model observes level, notes, current and previous score,
  current and previous miss, PGREAT, GREAT, GOOD, BAD, POOR, FAST, SLOW, and
  combo break in one dynamic batch. Its fixed input is BGR float32
  `N×3×32×320`; its dictionary is `0123456789-`; and its maximum sequence
  length is four.
- Model development uses operator-reviewed real crops only. Result frames are
  selected inside the screen episode containing each reviewed stable frame,
  crop digests are deduplicated globally before retaining at most 32 examples
  per field, and every episode remains in its capture-session split. A digest
  observed in multiple sessions is excluded from every split; a digest bound
  to conflicting field or label truth fails authoring. Permitted
  augmentation is limited to ROI jitter, brightness/color/contrast, blur,
  noise, and subpixel/downscale variation that preserves the label.
- Compare the official `en_number_mobile_v2.0_rec` MobileNetV3-small + BiLSTM
  initializer with the registered PP-OCRv6-small backbone and a mapped numeric
  head on the same five leave-one-session-out folds. A candidate is eligible
  only when calibrated typed acceptance makes no wrong holdout decision.
  Coverage of complete mandatory tuples, target batch latency, and bundle size
  then rank eligible candidates; MobileNet wins an otherwise equal comparison.
- The runtime scores every sequence in the field grammar with exact
  sequence-level CTC probability through the shared-prefix trie. It retains the
  top eight sequences, all-blank score, temperature-calibrated probability, and
  runner-up margin. A blank-winning or below-threshold crop stays `unknown`.
  Notes admits displayed leading zeroes; integer-valued score, judgment, and
  supplemental fields use canonical decimal sequences so an impossible leading
  zero cannot become a runner-up. The same logits also produce one deterministic
  unrestricted greedy CTC decode using Paddle's blank-first token order. It is
  retained only as raw diagnostic evidence and never participates in acceptance.
- Current score, PGREAT, and GREAT are selected jointly from their top-eight
  products under `score = 2 * pgreat + great`. Current and previous score must
  not exceed twice notes. PGREAT, GREAT, GOOD, and BAD individually must not
  exceed notes. No judgement-total relation to notes is imposed. POOR, current
  and previous miss, FAST, SLOW, and combo break have only display-width and
  integer bounds.
- Level and notes remain specialist observations when the result frame displays
  them. They are not fabricated from a background-only crop. Once the result
  song and difficulty are accepted, a unique single-play catalog chart may
  supply its registered level and notes when either displayed field is
  `unknown`; any known displayed value still constrains the match and a zero or
  multiple-chart match fails closed.
  This catalog-assisted rule advances the resolver to
  `scorepeek-result-fields-catalog-constrained-v4`.
- A complete mandatory numeric tuple becomes accepted after two equal
  observations in one result episode. Screen/session boundaries, time reversal,
  unknowns, and conflicting high-confidence tuples clear pending evidence. An
  accepted numeric result is retained for that episode and is rejoined with an
  accepted play attempt in either arrival order, emitting at most one existing
  `scorepeek-result-detected-v2` event.
- The active numeric model is a create-only private XDG data resource. Install
  verifies the manifest digest, ONNX and dictionary digests, fixed tensor
  contract, preprocessor, training generation, and calibration before atomic
  activation. Missing or changed resources fail `scorepeek run` closed; the
  general text model is never used as a numeric fallback.
- Recognition artifact v9 records the numeric model and preprocessor identity,
  batch latency, ranked candidates, blank score, calibration, typed state,
  joint decision, temporal state, and suppression reason. These are debug
  evidence. The accepted domain event remains the primary TUI and public socket
  surface and does not expose OCR candidates.

## Consequences

- Numeric recognition becomes independently trainable, calibratable, and
  replaceable without changing general text recognition or the v2 domain
  payload.
- One ONNX batch replaces fourteen sequential general-text inferences and lets
  the 10 Hz screen observer continue to own episode boundaries while the field
  worker is busy.
- Runtime startup now depends on an explicitly installed private numeric model.
  Model bytes, real crops, labels, and generated datasets remain outside the
  repository; source manifests, schemas, tooling, and reproducible export
  records remain reviewable here.
- Historical v5-v8 recognition artifacts and earlier diagnostic generations
  remain readable. They do not gain numeric-model authority retroactively.
- Public socket authority and target support remain unclaimed until the frozen
  model passes prospective target episodes with zero wrong mandatory accepts,
  one event per accepted attempt, no drops, and measured 14-field batch latency.

## Development evidence

- Operator review completed private suite v3
  `3ec72a21e55e65c4b5c5a6c386f10c47edcc60b41e5553ba870034d346764ea8`.
  Its globally deduplicated numeric dataset manifest is
  `696b072d59045f262eb45dc276542a09a27a44089c360b4c84ef35d8b3b7013a`:
  five capture-session splits, seventeen episodes, and 391 labelled samples
  with 391 unique crop digests. Exact crops repeated across sessions are
  excluded from all splits. Background-only level and notes crops are also
  excluded because the retained images contain no visible truth for them. The
  immutable prepared generation is
  `0afe2180ea751a8ce1dc71847ea2e57afe76c663be13f57cb363dbcbba2be954`.
- The registered PP-OCRv6-small backbone candidate is structurally ineligible
  for the fixed input contract. Its first held-out training run reaches feature
  height two and the registered backbone rejects the required height-three
  pooling kernel. Changing the approved `3x32x320` input or retaining the
  unrelated NRTR head would make it a different candidate, so no runtime model
  is selected from that path.
- MobileNet leave-one-session-out generation
  `791c77483a546b1ca630ca986fdc77a2bd490b8f2dd596e681c98fcd8fb066f1`
  produced 292 correct exact top candidates from 391 crops. Calibrated
  generation
  `9cd00e85cf7432c8903e54988af7fabbfba506024c8ce637c84c1e24357f8c50`
  selects field and joint boundaries from two equal adjacent observations in one
  episode. Judgment calibration operates on the complete five-value judgment
  tuple rather than independent field pairs. It accepts three correct tuple
  pairs, the score boundary accepts 29 correct pairs, supplemental accepts seven
  correct pairs, and the score joint accepts nine correct pairs, all with zero
  wrong accepts. Thirteen of sixteen complete holdout mandatory tuples pass every
  calibrated boundary. Level and notes calibration is explicitly disabled
  because no visible samples exist.
- Final training generation
  `b455bc5127bac1ae63a424497fdf0abf0016db22682dace9a756400c26533978`
  selected epoch 18. Fixed-width ONNX export
  `14a9888d53ada1acc93b320eb5d250d7a3bea2faf045151d2f20039f564051ef`
  produces model
  `b967ce0a6bdef2c8ea662d39a022926ca1ca904aae82cd8dd28980104a83deb1`.
  Paddle, ONNX, and Rust use the same `3x32x320` tensor contract; retained crop
  parity checks matched exact input tensor digests. Evaluation, final training,
  export, and runtime bundling additionally bind the initializer manifest and
  checkpoint, Paddle source commit, and training recipe.
- The `Horizons of Promise` retained frame contains no visible level or notes
  glyphs in those ROIs, so derived chart values are not sentinel truth. Visible
  sentinel generation
  `e560803fd26c0891d1225884bff7f0491d753a1a65c4641172f468e9eb33e641`
  requires score and all five judgments while treating the displayed
  supplemental/reference fields as optional values that may remain unknown but
  must never become a wrong known value.
- Sentinel evaluation
  `89f46deff580dda82ae7adccdf4489e66e223d3dc7671c07aa1e064159506775`
  accepts score `1383`, PGREAT `630`, GREAT `123`, GOOD `11`, BAD `0`, and POOR
  `2`, rejects the alternative score `1303` through the joint invariant, and
  produces no wrong optional known value. Its reviewed combo break truth is
  `1` and may safely remain unknown.
- Runtime manifest
  `7badce6d463a2d795e513b67979c9eceb53718adbcc7fa3b6afe4cbd12e1ba2a`
  binds the model, preprocessor, training/export generations, and temporal
  calibration. The active seventeen-episode v3 suite replays successfully with
  this bundle. Prospective target evidence remains a separate gate.
