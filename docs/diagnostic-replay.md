# Diagnostic canonical replay

## Retained full-frame reevaluation

`scorepeek diagnostic reevaluate` is the recognition path for an existing
`scorepeek-private-diagnostic-session-v4` or legacy v3. It verifies the exact source session and every retained
QOI it consumes, requires each QOI to remain a complete canonical RGB8 1920x1080 frame, then runs
the current production screen predicate and applicable registered OCR/catalog/result resolvers.

```bash
mise run diagnostic:reevaluate -- --session /absolute/private-session --session-sha256 SESSION_MANIFEST_SHA256 --output /absolute/new-evaluation
```

The active catalog and registered model/runtime are evaluator inputs, not inherited source truth.
The create-only output contains `observations.ndjson` and `manifest.json`, binding the source
session, evaluator executable, layout, catalog, model, and runtime. It also records whether the
catalog changed since capture.

The source QOIs are retained foreground evidence, not a newly recorded 10 Hz stream. Consequently
the command evaluates every retained full frame independently and explicitly reports
`session_reconstructed=false` and `temporal_domain_events_reconstructed=false`. It does not pass
the sparse sequence through temporal reducers or synthesize play attempts/domain events. Existing
retention cadence, quota, and source files are unchanged.

## Diagnostic writer replay

`scorepeek diagnostic replay` feeds digest-bound canonical RGB8 extraction
frames through the same bounded application worker and QOI diagnostic writer
planned for live capture. It does not execute recognition or reconstruct a game
session. The command is an offline evidence path, not a capture-support or
performance gate.

The request is a strict JSON object with no additional fields:

```json
{
  "schema": "scorepeek-diagnostic-replay-request-v1",
  "run_id": "ordinary-session-replay-001",
  "monotonic_start_ms": 0,
  "monotonic_end_ms": 1016,
  "build_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
  "capture_generation": 1,
  "capture_profile_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
  "normalizer_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
  "canonical_layout_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
  "catalog_sha256": "5555555555555555555555555555555555555555555555555555555555555555",
  "model_sha256": "6666666666666666666666666666666666666666666666666666666666666666",
  "runtime_sha256": "7777777777777777777777777777777777777777777777777777777777777777",
  "extraction_sha256": "8888888888888888888888888888888888888888888888888888888888888888",
  "frames": [
    {
      "sequence": 1,
      "frame_id": "ordinary-000",
      "monotonic_start_ms": 0,
      "monotonic_end_ms": 0
    },
    {
      "sequence": 2,
      "frame_id": "ordinary-001",
      "monotonic_start_ms": 1000,
      "monotonic_end_ms": 1000
    }
  ]
}
```

The caller supplies the SHA-256 of the exact request bytes. Frame sequence and
start time must increase by at least the fixed 1,000 ms sampling interval; frame
end must equal that instantaneous source time; every frame ID must be unique and
advance in extraction decode order; every frame
must remain inside the declared run boundary. The extraction must be a strict
`scorepeek-private-canonical-frame-extraction-v1` accepted by
`CanonicalFrame::read_extraction`. Its capture profile, normalizer artifact,
and extraction digest must match the request.
The extraction contract fixes a 1/1,000 source time base, and each request start
must equal that frame's non-negative `source_pts`. Caller-authored timing cannot
replace extraction timing.

```bash
mise run diagnostic:replay -- --request /absolute/request.json --request-sha256 REQUEST_SHA256 --extraction /absolute/canonical-extraction --output-root /absolute/existing-diagnostic-root
```

The output root must already exist. The run directory is create-only. Public
stdout contains one value-free JSON summary with the run ID, request digest,
offered/enqueued counts, completeness, error type, and manifest digest. Pixels,
frame IDs, OCR values, and local paths remain only in the private input or run.
The command exits successfully only for `complete` with a published manifest;
invalid descriptors, partial/dropped completion, and a missing manifest exit
non-zero without printing a success summary.

Live producers use a non-blocking queue offer and record `queue_full` without
changing recognition. The producer applies the 1 Hz cadence gate before queue
admission, and the writer checks it again. The offline replay may retry queue admission for at most
five seconds per frame and uses the same bounded five-second finish. A missing manifest, `partial`,
or `dropped` run cannot prove a result episode was absent. `flush_timeout` means
the caller stopped waiting; a thread already in filesystem publication may
still publish the authoritative manifest later, while the single-worker
supervisor rejects new runs until that thread exits.
