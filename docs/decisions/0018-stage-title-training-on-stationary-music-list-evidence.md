# ADR 0018: Stage title training on stationary music-list evidence

- Status: Accepted
- Date: 2026-08-20
- Complements: ADR 0016

## Context

The product's non-negotiable title-recognition outcome is to avoid missing result records. Result
screens are nevertheless expensive to collect deliberately because each distinct sample requires a
play. Data-collection-only play sessions are burdensome and are not an acceptable prerequisite for
iterating on the title model.

The current private music-list corpus provides much broader evidence at low collection cost. It has
1,119 songs represented by 3,061 deduplicated, complete, stationary, non-selected, standard-color
row crops. ADR 0016 establishes why those rows may provide provisional evidence for the thin result
title renderer while preserving their music-list origin. Scrolling, selected, clipped, obscured,
locked/dimmed, and non-standard color observations remain separate states or presentation domains.

Treating the small result corpus as a prerequisite would stall useful model work. Treating
music-list accuracy as proof of result accuracy would instead optimize a surrogate without proving
the product outcome. The sequencing and evidence roles therefore need to be explicit.

## Decision

- Improve the title model against the complete eligible stationary standard music-list corpus
  before requiring a large result corpus. The primary current optimization metric is strict growth
  of the set of corpus songs whose every available eligible crop resolves to the correct unique
  catalog song ID, without losing a previously fully recognized corpus song.
- Retain title-disjoint validation and evaluation as generalization guards. A candidate may improve
  known-corpus coverage without increasing either split, but it must not turn an already fully
  recognized held-out song into an incomplete one.
- Music-select live recognition is required only after scrolling has stopped and the relevant
  screen state has remained stable for the calibrated dwell. Recognizing titles during scrolling
  is not an acceptance requirement; transitions remain unknown and emit no song decision.
- The selected central music-list title remains a separate rendering domain from the current
  non-selected row corpus. Do not claim selected-song live support until stable selected-title
  evidence passes its own replay gate.
- Collect result observations opportunistically from ordinary live play rather than requesting
  dedicated result-data play sessions. The evidence source must preserve an independently
  reviewable session timeline that can enumerate eligible result episodes even when the result
  detector, OCR, or event path emits nothing; those recognition paths cannot be the sole collection
  trigger. A bounded continuous or rolling capture-profile recording may provide that timeline.
  Preserve result frames/crops, timing and session provenance, model/catalog/profile digests,
  candidate scores, and emitted, suppressed, or absent decision state as private evidence for later
  replay and independent labeling.
- Never promote the model prediction itself, or a prediction derived from the same model family, to
  accepted result truth. Natural live observations may be batch-reviewed or independently labeled
  later without interrupting ordinary play.
- Partition passively collected result evidence by title, session, and play into a development
  transfer-sentinel set and an accepted result holdout that remains frozen and unavailable to model,
  threshold, and candidate selection. Replay every viable music-list candidate against the sentinel
  set. A music-list gain is not a result-accuracy claim, and a candidate that regresses an already
  correct sentinel decision does not advance toward runtime selection. Sentinel performance is not
  release accuracy.
- Final runtime and release acceptance remains result-centered. It must measure the complete path
  from result-screen detection through unique song resolution, event emission, session handling,
  and deduplication on the frozen accepted holdout or prospective ordinary sessions collected only
  after the candidate and thresholds are fixed. The independent session timeline supplies the
  result-episode denominator so title accuracy or detector-triggered collection cannot hide a
  result-record omission elsewhere in the pipeline.

## Consequences

- Work may continue on the five incomplete stationary music-list songs without waiting for more
  result screens, using only train-split crops for optimization and the complete 1,119-song census
  for selection.
- The current music-list winner is a surrogate-domain winner, not a selected runtime result model.
  Result confidence and rejection thresholds remain uncalibrated until ordinary live sessions
  accumulate sufficient negative, transition, similar-title, and distinct-result evidence.
- Result corpus growth is passive during normal use but evidence capture and replay must be designed
  before claiming integrated result support. Early evidence may all remain development-only when
  too little exists to freeze a meaningful holdout; release acceptance then waits for later
  prospective ordinary sessions rather than reusing the sentinel set. Missing evidence is reported
  as an unverified boundary, not converted into a requirement for dedicated play.
- Music-select scrolling recognition can remain unimplemented. Stable music-select recognition is
  still useful for selected-song records, but its selected-title domain and dwell gate are separate
  from the stationary row training surrogate.
