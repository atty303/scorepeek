# ADR 0025: Record bounded application-owned live diagnostic runs

- Status: Accepted
- Date: 2026-08-22
- Complements: ADR 0024's minimal selection song context and the
  recognition-independent recording rationale retained from ADR 0023

## Context

Scorepeek must make a missed result observable even when screen detection,
OCR, song resolution, and event delivery all fail to trigger. Recognition-led
artifact capture cannot provide that denominator. At the same time, the
diagnostic path must not reconstruct INFINITAS mode, attempts, play counts,
retry counts, or a full game session.

The current canonical contract is RGB8 1920x1080. Uncompressed PPM is
6,220,817 bytes per frame and makes a default rolling recorder unnecessarily
expensive. A bounded probe over existing private canonical frames measured
43,545,719 bytes for seven PPM frames, compared with 10,354,554 bytes for PNG,
11,398,429 bytes for QOI, and 14,173,137 bytes for zstd-compressed PPM. A
thirty-frame FFV1 stream was 19,831,101 bytes instead of 186,624,510 bytes of
PPM, but it would add child-process, pipe, flush, segment, and crash-recovery
ownership to the game-session runtime.

## Decision

The scorepeek application owns a diagnostic run outside the recognition
library and public event stream. One run covers one immutable
capture-generation binding. A capture/profile/normalizer/layout/catalog/model
or runtime binding change ends the run and starts another; this is an
application resource boundary, not an inferred game session.

The result, operation, and observation surfaces remain separate:

- the public result remains the versioned accepted-event stream;
- application controls start, opt-out, status, freeze, delete, and create-only
  local export;
- private diagnostic records retain operations, typed status/error, observed
  screen facts, song-context changes, song decisions, event outcomes, and
  canonical-frame artifacts under one opaque run ID.

The independent sampler is offered every canonical frame before recognition
outcomes are known. Its initial policy retains at most one sample per 1,000 ms.
This cadence is provisional evidence collection, not a support threshold. A
run is eligible as a result-miss denominator only after a minimum result dwell
has been calibrated from more than one representative recording and the
run's measured maximum observation gap is strictly below that dwell. Until a
create-only calibration artifact binds its provenance, recording count, and
minimum dwell, every run records denominator eligibility as `false`. Sequence
or timing regressions, queue or capacity drops, and missing artifacts make the
affected run `partial` or `dropped`; they never prove result absence.

Canonical samples use QOI as independently decodable, lossless RGB artifacts.
The approved runtime dependency is `qoi` 0.4.1 (MIT OR Apache-2.0), with its
normal `bytemuck` dependency. QOI was selected over PNG for a smaller pure-Rust
surface and over FFV1 for per-frame create-only publication and simpler crash
recovery. Compression ratio from the bounded probe is feasibility evidence,
not a performance or capacity gate.

Each run publishes:

1. `run.json` first, binding the resource, immutable recognition inputs,
   monotonic run start, and recording policy;
2. create-only, digest-bound QOI frames and strict bounded fact documents;
3. `manifest.json` last, digest-binding `run.json` and all artifacts, with
   status, `complete | partial | dropped`, the maximum leading, adjacent, or
   trailing unobserved interval through the explicit monotonic run end, bounded
   reason-bearing missing ranges and truncation counts, artifact/manifest/total
   byte counts, and denominator eligibility.

A directory with `run.json` but no completion manifest is recoverable evidence
of a partial run. All fallible finalize preparation precedes manifest
publication; that create-only publication is the final commit point.
Publication is no-clobber and fsync-complete. Local access
control remains operator-owned under ADR 0014; pixels, player/rival UI, OCR
text, and local paths remain outside commits and the public NDJSON stream.
Remote export is disabled and is not implied by local create-only export.

The default aggregate local retention budget is 8 GiB. Completed normal runs
have a 24-hour grace period; program-error, timeout, crash, and operator-frozen
runs have a seven-day priority period. Retention first removes expired normal
runs and then the oldest non-priority normal runs. It never removes an active
run to admit a sample. When only active or priority data remains at capacity,
new diagnostic data drops and recording health degrades without changing
recognition, event output, exit status, or other application state. A single
run is additionally bounded to 8 GiB, 8,192 frames, 32,768 fact documents, and
64 KiB per fact.

The live writer will receive samples through a bounded non-blocking application
queue. Encoding, storage, flush, and export failures belong to the diagnostic
operation once and do not replace the recognition result. The initial vertical
slice implements the strict storage writer, cadence, bounds, start-document
verification, manifest-last completion, opt-out, QOI round-trip,
operation-scoped typed status/error consistency, bounded degradation ranges,
and explicit degradation-log truncation. Queueing,
retention management, user controls, live capture integration, and target-host
performance remain later slices.

## Consequences

- Sparse full frames remain replayable while screen predicates and ROI needs
  are still changing. Moving later to a padded ROI atlas requires
  byte-equivalent decision replay against the retained full-frame contract.
- Diagnostic facts describe observations and decisions but do not become a
  second recognition state machine.
- A green storage replay does not establish calibrated cadence, target-host
  performance, capture-profile support, result recall, or public event
  compatibility.
- The application, not `SongContext` or the recognizer, owns retention,
  deletion, export, and recording health.
