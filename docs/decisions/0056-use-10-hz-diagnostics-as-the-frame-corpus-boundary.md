# ADR 0056: Use 10 Hz diagnostics as the frame-corpus boundary

## Status

Accepted

## Decision

Live Gamescope capture and offline video replay feed the same production normalizer, scene
predicate, OCR, and catalog resolver at a fixed 10 Hz recognition cadence. Live capture keeps only
the latest available frame; it never drains a backlog. A recognition worker that is still busy
causes a counted `busy_skip`, not session degradation. Offline video sampling is based on source
time and deterministically selects the latest decoded frame at each 100 ms tick.

`scorepeek-private-diagnostic-session-v3` is the sole intermediate form accepted by the capture
regression corpus. It contains ordered tick/source timestamps and sampling completeness, one
`facts.ndjson`, one `observations.ndjson`, ordered QOI evidence references, and component digests.
Canonical evidence is RGB8 1920x1080 QOI. Optional observed evidence is RGB8 QOI with the original
BGRx contract retained as metadata; new diagnostics do not retain uncompressed BGRx. Evidence is
content-deduplicated and bounded independently of the complete fact and observation streams.
Video replay stops adding image references at 1,024 frames or 1 GiB of unique encoded QOI bytes,
records the capacity transition, and continues recognition and both NDJSON streams.
Each fact, observation, or event record is limited to 1 MiB and each session stream is limited to
250,000 records. Reaching a stream bound is explicit degradation rather than an unbounded write.
Value-bearing observations retain exact OCR fields, the resolver decision and its selected or
runner-up evidence, plus a candidate count bound to the separately stored exact catalog table.
They do not duplicate every recomputable per-song metric into every tick; this keeps the complete
stream within 512 MiB for the full 250,000-tick session bound.

Only a verified v3 diagnostic may be imported. Import creates an immutable capture-session object
and a review draft; recognizer output is never promoted to truth. An operator-applied immutable
label records inclusion, episode boundaries, stable frames, song identity, clear type, and explicit
negative frames. Successful review publication alone creates and atomically activates a new suite
generation. Replay applies the production predicate to every stored canonical frame, applies
production normalization to every stored observed/canonical pair, and runs OCR, catalog resolution,
and clear-type resolution on operator-labeled stable frames. It does not run a wall-clock scheduler.

Video is an optional diagnostic input and auxiliary provenance object, not a corpus dependency.
The new store is a clean cut: capture regression readers do not dual-read recording roots, old
datasets, or videos. Existing artifacts remain read-only archives and may be converted once through
the explicit v2 converter.

The result predicate continues to require the warm header and upper panel edge. The lower edge is
retained as diagnostic evidence but is not an acceptance condition because retained stable result
frames showed it was neither stable nor discriminating.

The reviewed four-result live diagnostic also fixes two fail-closed recognition details. Registered
clear types accept an exact value or a unique ASCII edit-distance-one value, which recovers the
measured `XH-CLEAR` OCR without permitting an unregistered label. Result-song resolution keeps title
as the primary field and artist as corroboration, but permits at most three title edits and requires
at least three quarters normalized title similarity. The existing unique-best and runner-up margin
requirements remain; these bounds admit the reviewed `Miracle Sumpho` observation without adding a
fuzzy fallback or promoting diagnostic output to truth.

## Consequences

- 60/120 fps source acceptance is independent from 10 Hz recognition cost.
- Missing, dropped, and busy ranges remain explicit and cannot become implicit negative labels.
- Live play and saved video converge before review, while corpus replay remains deterministic and
  video-independent.
- The former recording-dataset import/seal/transfer CLI is no longer a capture-regression entrypoint.

## Supersedes

This decision supersedes the video-required capture-corpus, recording-root dataset, direct-video
import, per-fact JSON, foreground-compacted ordinary-run retention, and per-observation duplication
of the complete recomputable catalog metric table from the earlier corpus and diagnostic decisions.
OCR training corpora and their independently licensed inputs are unchanged.
