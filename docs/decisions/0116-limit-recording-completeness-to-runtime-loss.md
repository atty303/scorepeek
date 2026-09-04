# ADR 0116: Limit recording completeness to runtime loss

Status: Accepted

## Context

The recorder independently revalidated application-owned typed facts. MUSIC SELECT added fields
while that validator retained an older field count, so successful observations were discarded as
`invalid_configuration`. The diagnostic session became partial despite complete canonical video
and event streams. This duplicated the producer's schema and hid an implementation defect as
recording loss.

## Decision

`complete`, `partial`, and `dropped` describe persistence at runtime, not whether application
inputs, recognition, or internal implementation contracts are correct. This supersedes ADR 0025's
classification of internal sequence/timing regressions as recording loss, ADR 0026's corresponding
producer degradation checks, and ADR 0032's independently validated field-count schema.

Keep runtime loss from queue/backpressure, capacity and memory limits, missing admitted sequence
ranges, writer/encoder failures, unavailable workers, abandoned admitted work, flush/finalization
timeouts, and interrupted publication. Normal intentional sampling is not loss. Operation errors
are independent: rejecting an invalid replay input can have an error status and complete diagnostic
persistence.

Persist typed application facts without a second semantic validator. Internal frame shape, run
binding, and chronology preconditions are assertions rather than recoverable recording drops.
Canonical chronology regressions do not start a synthetic reset/recovery segment. Rejecting a
foreign pending job does not degrade a run which never admitted that job. A worker that actually
terminates still represents runtime unavailability; no completion is fabricated after a crash.

External input schema/digest/path validation and runtime resource limits remain enforced. Existing
reason names remain readable for historical recordings, but new producers do not emit
`invalid_configuration`, `sequence_nonmonotonic`, `timing_nonmonotonic`, or `chronology_reset` as
recording-loss reasons. Existing private sessions are not rewritten or relabeled complete.

## Consequences

A new SELECT field no longer requires changes to a second recorder schema. Regression tests cover
persisting the eight-field SELECT summary, foreign-job rejection without loss, invalid replay
input separately from recording loss, and existing queue/capacity/write/finalization failures.
Import and regression-suite admission remain unchanged; this decision does not authorize remote
upload or recovery of missing historical records.
