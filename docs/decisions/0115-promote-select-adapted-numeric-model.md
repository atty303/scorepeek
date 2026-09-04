# ADR 0115: Promote SELECT-adapted numeric weights

- Status: Accepted
- Date: 2026-09-04
- Supersedes: ADR 0094 for the registered numeric weights; ADR 0114 for reuse of the unchanged
  RESULT-trained weights in MUSIC SELECT

## Context

The registered classifier identifies the sampled SELECT digit 6 but its 0.557 logit margin is below
the fixed 1.0 acceptance gate. Additional training improves the observed fields without relaxing
that gate. The operator approved adoption subject to production-path SELECT/RESULT replay.

## Decision

- Register runtime manifest artifact v3, retaining runtime contract schema v2, the HOG/MLP
  architecture, fixed layouts, preprocessing and all calibration thresholds.
- Adopt model `05d2bc903bcd7e36e3c62402d2a10b59d7e5a4141a6603481bcd6b828fc66b3e`, initialized from
  ADR 0094's weights. Its SELECT training uses the old/new SP groups; DP frames are excluded from
  SELECT supervision. Seed 0, Adam at 0.0001, weight decay 0.0001 and 40 epochs are fixed. Cross
  entropy uses the existing five augmentations; an MSE term retains the parent model's logits on
  the retained RESULT crops (256 cells per step, coefficient 1).
- Use Rust's production `fixed_slot_feature` for offline features. The existing Python reference
  differs on some crops; the retained experiment verifies production values before training.
- Bind parent model, SELECT labels, RESULT source dataset, layouts, training recipe and export
  verification through `numeric-fixed-slot-select-adaptation-v1.json`. Complete labels, input
  images, training scripts containing private paths, evaluation details and ONNX remain private.
  The private bundle includes digest-bound `inputs.json`, `training.json`, and `evaluation.json`.
- Keep previous manifests immutable. New binaries require the new registered bundle through the
  existing create-only installer. This does not authorize deployment or changes to a live store.

## Evidence and limits

The three session-group comparison folds improve SCORE 11/15 to 15/15 and MISS 12/15 to 15/15,
with zero wrong accepted fields and unchanged header/dash predicates. Independent groups contain
identical glyph pixels: each fold shares 8–9 glyphs with evaluation. This is adaptation evidence,
not unseen-glyph generalization. Excluding every identical glyph leaves only 1–3 training glyphs
and does not produce an acceptable model.

The adopted fold has 11 unique glyph crops and 55 augmented inputs. Its RESULT retention check
uses 806 source field crops, including transitions; correct-to-wrong changes are zero. Since those
crops also provide teacher logits, this check does not replace production replay. ONNX readback
agrees with Paddle to within 2.39e-6 logits. Current production-path verification is in `STATUS.md`.

Additional capture conditions, four-digit MISS, and target-live performance remain unverified.
Best snapshots remain supplemental observations and never become RESULT events or identity evidence.
