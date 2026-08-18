# ADR 0015: Use provisional private title data during development

## Status

Accepted

## Context

ADR 0006 restricted external catalog strings to inference lexicons and required independently
licensed or generated training text. That is a safe release boundary, but it prevents the current
private recording from contributing efficiently to recognizer development: the recording contains
only two result screens while each music-select frame exposes many exact title rows.

The operator intends to obtain upstream permission. During development, these inputs remain local
and are not distributed as repository or release artifacts.

## Decision

This decision supersedes ADR 0006 only for the source policy of private development data.

- Private development corpus generations may contain real game title crops and provisional title
  text derived from a provenance-bound active catalog, including visible music-select list rows.
- Every provisional label records the catalog digest, source lineage or revision, crop digest, and
  permission status. It is never silently converted into redistribution evidence.
- Automated exact catalog association may prepare provisional training data. Accepted holdout
  labels, recognition thresholds, and release gates still require human confirmation and remain
  title/session/play-disjoint.
- Models trained from provisionally permitted inputs remain private development artifacts. They
  cannot be promoted to a distributable bundle or release gate until the relevant permissions and
  licenses are recorded for every contributing generation.
- Upstream code, coordinates, resources, and generated artifacts remain outside the repository.
  This decision does not weaken ADR 0004 or make another implementation a pixel/layout reference.
- Runtime inference remains offline and fail closed. This decision does not permit runtime
  self-training, model auto-download, automatic threshold relaxation, or raw crop/event export.

## Consequences

- The current recording can supply many distinct title observations before more result recordings
  exist.
- License status becomes explicit corpus/model provenance rather than a reason to discard useful
  local development evidence.
- A successful private experiment is not, by itself, evidence that its dataset or model may be
  published.
