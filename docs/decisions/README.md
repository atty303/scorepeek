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
- [ADR 0007: Select one post-scale capture backend from Bazzite evidence](0007-select-capture-on-bazzite-evidence.md)
  supersedes ADR 0002.

## Historical

- ADR 0001: upstream release/resource adoption
- ADR 0002: separate FHD OBS WebSocket and Gamescope profiles
- ADR 0003: Python upstream-resource importer and Rust runtime

Historical ADRs describe the initial bootstrap design and are not implementation
requirements after their named superseding decisions.
