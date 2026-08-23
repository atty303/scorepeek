# ADR 0035: Run bounded Gamescope field observation without publishing values

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0033's integrated application owner and ADR 0034's full-catalog evidence

## Context

The registered field worker and live recognition session already share one immutable descriptor,
but the production observer previously returned only raw field observations. The full-catalog
candidate domain existed only as a pure component, and no executable Gamescope boundary joined
capture, normalization, classification, registered inference, candidate scoring, and ordered
diagnostic finalization.

That boundary must be observable without turning OCR strings, catalog records, candidate scores, or
pixels into a public result. It is an evidence gate, not an accepted-field or supported-profile
claim.

## Decision

The registered screen-field observer constructs one `CatalogCandidateDomain` from the already
loaded active catalog before its worker starts. A search-term-only catalog song fails startup with a
typed candidate-domain error. Each successful worker output contains the complete screen field set
and the corresponding all-song candidate observations together. The worker does not rank, truncate,
accept, persist, or emit either value.

`gamescope-field-observation-gate` owns one bounded invocation. Its descriptor binds the admitted
capture generation, profile, normalizer, canonical layout, catalog, model, runtime, and diagnostic
run. The CLI additionally selects the catalog store and registered model bundle used by the
pre-worker loader. Registered resources and the candidate domain finish loading before Gamescope
provider acquisition or receiver processing begins. Capture and inference have no automatic
fallback.

The capture loop remains non-blocking. It normalizes the latest admitted frame, classifies the
screen, submits complete field crops when applicable, and polls only application-owned pending
handles. Queue or outstanding-capacity rejection remains separate from the screen observation.
After the requested capture duration, the gate shuts down the receiver and provider, waits at most
the existing bounded field-worker finish interval for selected pending results, closes the field
worker, and only then finalizes the diagnostic run.

Success requires at least one completed field observation and its full-catalog candidate set. An
unknown-only run, inference failure, worker loss, binding mismatch, capture failure, teardown
failure, or incomplete worker finish is a typed gate error. This is not a recognition-accuracy or
candidate-acceptance threshold.

The public JSON contains only stable status/error categories, screen and lifecycle counts,
candidate-set and scored-candidate counts, bounded field-worker completion counts, diagnostic
completeness/manifest identity, and the existing bounded typed capture facts. It contains no OCR
text, catalog strings, song IDs, per-song scores, pixels, resource or binding bodies, filesystem
paths, environment/session strings, arbitrary PipeWire properties, or raw external errors. These
values are absent because they are not needed by this result contract, not because ordinary local
catalog or path values are credentials.

Diagnostic recording remains independently optional. Enabled recording retains the existing
canonical-frame, screen, field-count, failed-field, degradation, and lifecycle facts; it does not
record candidate values. Disabled or degraded recording cannot change field inference or candidate
scoring.

## Consequences

- The production worker now exercises ADR 0034 on every successful registered field result.
- A bounded development or private live run can distinguish acquisition, normalization,
  classification, submission, inference/scoring, worker finish, and diagnostic finalization.
- Candidate evidence remains in-process and is discarded by this gate after value-free counting.
- The gate does not define ranking, field acceptance, song resolution, temporal agreement, event
  authority, performance acceptance, or capture-profile support.
- Private INFINITAS verification and target-host performance remain separate evidence boundaries.
