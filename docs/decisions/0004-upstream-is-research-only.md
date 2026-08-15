# ADR 0004: Treat the Windows implementation as research only

- Status: Accepted
- Date: 2026-08-15
- Supersedes: ADR 0001

## Context

ADR 0001 separated Git history but still adopted upstream releases, resources,
and catalogs. That preserved the visual-domain mismatch, pickle trust boundary,
and upstream data coupling that the independent repository was intended to
remove.

## Decision

The Windows implementation may be inspected once to identify areas worth
measuring. No code, coordinate table, `.res`, image, music database, pickle, or
derived artifact enters scorepeek.

Committed layout values are independently measured from scorepeek's private
Linux captures. Game-content updates come through separately governed online
IIDX sources and scorepeek's own recognition corpus.

## Consequences

- There is no upstream release adoption workflow or resource importer.
- Upstream layout changes are detected through live behavior and corpus replay,
  then handled as scorepeek layout or recognizer changes.
- Capture, catalog identity, recognition, artifacts, and APIs are scorepeek
  responsibilities.
