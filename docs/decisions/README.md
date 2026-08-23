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
- [ADR 0019: Apply comparison keys to catalog-constrained CTC candidates](0019-apply-comparison-keys-to-ctc-candidates.md)
  supersedes ADR 0006 only for exact-only catalog candidate sequences.
- [ADR 0020: Select an official ONNX recognizer before custom training](0020-select-official-onnx-before-custom-training.md)
  supersedes ADR 0006 for its mandatory fine-tuning/custom-export sequence and ADR 0018 only for
  model candidates requiring set-inclusion growth.
- [ADR 0021: Search the full song catalog from imperfect text observations](0021-search-the-full-song-catalog-from-imperfect-text.md)
  supersedes ADR 0020 only for its direct-encodability or derived-signature evaluation gate.
- [ADR 0022: Select PP-OCRv6 small for contextual song recognition](0022-select-pp-ocrv6-small-for-contextual-recognition.md)
  supersedes ADR 0006's mandatory custom/single-title sequence, ADR 0020's exhaustive phase-two/no-selection requirement, and ADR 0021 only for
  requiring every decoder policy to be compared across every model.
- [ADR 0024: Limit temporal state to selection song context](0024-limit-state-to-selection-song-context.md)
  supersedes ADR 0023's `play_attempt` and full-session state inference while retaining its
  screen-context and recognition-independent recording rationale, and supersedes ADR 0022 only for
  naming play-attempt transitions as the contextual integration gate.
- [ADR 0025: Record bounded application-owned live diagnostic runs](0025-record-bounded-live-diagnostic-runs.md)
  fixes the diagnostic run, storage, completeness, retention, privacy, and non-interference contract
  that ADR 0023 deferred while keeping ADR 0024's minimal recognition state.
- [ADR 0026: Isolate diagnostic I/O behind a bounded application worker](0026-isolate-diagnostic-io-behind-a-bounded-worker.md)
  fixes queue ownership, producer-side cadence, non-blocking live offers, bounded flush, and strict
  canonical replay.
- [ADR 0027: Acquire PipeWire sources behind a common receiver](0027-acquire-pipewire-sources-behind-a-common-receiver.md)
  fixes the source-provider/receiver boundary, selects Gamescope as the first direct PipeWire spike,
  defers Portal to a later provider without automatic fallback, and supersedes ADR 0009 and ADR 0013
  only for treating a future OBS path as an eligible scorepeek capture profile. ADR 0013's existing
  offline OBS/vkcapture recording profile remains valid.
- [ADR 0028: Build PipeWire against a mise-pinned SDK](0028-build-pipewire-against-a-mise-pinned-sdk.md)
  fixes the Linux x86-64 host-native Cargo boundary: mise provides the checksum-pinned PipeWire SDK
  and native pkgconf executable, while `cc`, libclang with matching resource headers, and the
  PipeWire runtime remain explicit host prerequisites. Python, containers, and Zig are not added.
- [ADR 0029: Bind capture profiles after source acquisition](0029-bind-capture-profiles-after-source-acquisition.md)
  supersedes ADR 0027 only for its profile-bearing initial lease and combined lifecycle ownership.
  Providers first return an uncalibrated lifetime lease; only an explicit immutable calibration
  binding lets the receiver emit profile-bearing `ObservedFrame` values.
- [ADR 0030: Isolate live field observation behind a run-bound worker](0030-isolate-live-field-observation-behind-a-run-bound-worker.md)
  fixes the application-owned loader, queue, provenance, result, and finish boundary between live
  screen crops and future model/catalog observers without defining accepted field values.
- [ADR 0031: Load the registered live text runtime once](0031-load-the-registered-live-text-runtime-once.md)
  binds the active catalog, PP-OCRv6-small bundle, and fixed CPU runtime manifest to one synchronous
  pre-worker loader without granting field, song, or event authority.

## Historical

- ADR 0001: upstream release/resource adoption
- ADR 0003: Python upstream-resource importer and Rust runtime
- ADR 0008: route-local normalizers mapped to an underdetermined conceptual
  canonical frame and source ingest prematurely bound layout
- ADR 0023: explicit play-attempt linkage and full-session timeline proposal

Historical ADRs describe the initial bootstrap design and are not implementation
requirements after their named superseding decisions.
