# ADR 0082: Re-evaluate retained full-frame diagnostics

- Status: Accepted
- Date: 2026-08-30
- Complements: ADR 0056, ADR 0075, and ADR 0081

## Context

Routine diagnostics retain a bounded foreground and failure window as canonical RGB8 1920x1080
QOI frames. The complete frame is intentionally more durable than the crop set or predicate that
happened to be implemented when the run was recorded: later screen layouts, OCR fields, and
catalog resolvers may need pixels outside an old crop.

The existing `diagnostic replay` command only exercises the diagnostic QOI writer. The
single-frame `recognition inspect-diagnostic-qoi` command runs the current screen predicate but not
the registered field observer. Neither command can evaluate all retained frames against the current
production recognition resources, so investigations have required ad hoc frame copying and cannot
produce one source- and evaluator-bound result.

Foreground retention is deliberately sparse. In particular, known screens are retained at a
slower cadence than temporal song and result policies consume. Replaying those sparse frames as if
they were the original 10 Hz stream would manufacture temporal resets, missing attempts, and
unlinked results.

## Decision

Add `scorepeek diagnostic reevaluate` for an operator-selected
`scorepeek-private-diagnostic-session-v3`. The command:

- digest-binds and validates the source session manifest, capture manifest, capture start binding,
  and every retained canonical frame it consumes;
- accepts only full canonical RGB8 1920x1080 QOI evidence, never a crop or a source-resolution
  frame;
- runs every retained canonical frame through the current production screen predicate and, when
  applicable, the current registered OCR, catalog resolver, and result-field resolver;
- uses the currently active catalog and registered model/runtime, recording their identities and
  whether the catalog differs from the source run;
- publishes one create-only private evaluation directory containing ordered NDJSON observations
  and a digest-bound manifest; and
- records coverage as `retained_full_frame_qoi`, the source processed/busy counts, and explicit
  `session_reconstructed=false` and `temporal_domain_events_reconstructed=false` markers.

The source diagnostic and its QOI files remain immutable and are not copied into the evaluation.
The evaluation manifest links the source session SHA-256, evaluator executable SHA-256, canonical
layout, catalog, model, and runtime. A failure before complete publication leaves no evaluation at
the requested destination. The output must resolve outside the source session. An output-specific
writer lock, atomic directory reservation, and owned staging marker prevent replacement by a
concurrent writer and let the next invocation recover an interrupted incomplete destination;
removing that marker is the publication commit point.

Keep the existing foreground retention cadence and aggregate capacity. This decision does not
retain every 10 Hz recognition tick. Complete QOI means an uncropped canonical frame, not complete
tick coverage.

## Consequences

- Screen, crop, OCR, and resolver changes can be evaluated later against every retained full frame
  without pretending that the old crop layout remains authoritative.
- A catalog change is visible and reproducible as an evaluator-resource change rather than being
  silently attributed to recognition code.
- The derived observation stream is valid per retained frame, but it is not evidence of the exact
  temporal or play-attempt path the original session would take under current code.
- Exact current temporal/domain-event reconstruction still requires an input stream at the
  production cadence and is outside this bounded foreground diagnostic contract.
