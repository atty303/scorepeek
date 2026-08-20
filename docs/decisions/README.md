# Architecture decision index

This index resolves the current decision set without rewriting accepted ADRs.
When an older ADR conflicts with a superseding decision, the newer ADR is
authoritative.

## Current

- [ADR 0004: Treat the Windows implementation as research only](0004-upstream-is-research-only.md)
  supersedes ADR 0001.
- [ADR 0005: Federate external IIDX catalogs without fuzzy identity merging](0005-federate-external-catalogs.md)
- [ADR 0006: Train sequence OCR offline and run catalog-constrained inference in Rust](0006-train-sequence-ocr-run-rust.md)
  supersedes ADR 0003.
- [ADR 0009: Own game layout in the canonical frame contract](0009-own-layout-in-the-canonical-frame-contract.md)
  supersedes ADR 0008.
- [ADR 0010: Preserve recordings as reusable dataset roots](0010-preserve-recordings-as-reusable-dataset-roots.md)
- [ADR 0011: Index FFV1 recordings by packet order](0011-index-ffv1-recordings-by-packet-order.md)
  supersedes ADR 0010's decoded-frame probing method.
- [ADR 0012: Allow path-backed private source objects](0012-allow-path-backed-private-source-objects.md)
  supersedes ADR 0010's mandatory local source copy.
- [ADR 0013: Bootstrap the shared layout from a normalized profile](0013-bootstrap-layout-from-a-normalized-profile.md)
  supersedes ADR 0009's multi-profile-first sequencing requirement.
- [ADR 0014: Delegate local filesystem permissions to the operator](0014-delegate-local-filesystem-permissions.md)
  supersedes ADR 0010 and ADR 0012 only for local filesystem mode policy.
- [ADR 0015: Use provisional private title data during development](0015-use-provisional-private-title-data-during-development.md)
  supersedes ADR 0006 only for private-development training-data source policy.
- [ADR 0016: Use stationary music-list rows as result-title evidence](0016-use-stationary-list-rows-as-result-title-evidence.md)
- [ADR 0017: Separate music-list title presentation domains](0017-separate-music-list-title-presentation-domains.md)
- [ADR 0018: Stage title training on stationary music-list evidence](0018-stage-title-training-on-stationary-music-list-evidence.md)

## Historical

- ADR 0001: upstream release/resource adoption
- ADR 0003: Python upstream-resource importer and Rust runtime
- ADR 0008: route-local normalizers mapped to an underdetermined conceptual
  canonical frame and source ingest prematurely bound layout

Historical ADRs describe the initial bootstrap design and are not implementation
requirements after their named superseding decisions.
