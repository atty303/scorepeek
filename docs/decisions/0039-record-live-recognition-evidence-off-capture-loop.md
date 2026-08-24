# ADR 0039: Record live recognition evidence off the capture loop

- Status: Accepted
- Date: 2026-08-24
- Complements: ADR 0026's bounded I/O isolation, ADR 0033's application-owned field session,
  ADR 0037's value-bearing local evidence, and ADR 0038's result resolver

## Context

The Gamescope field gate already produced the same registered field and result-song resolution
objects as recording simulation, but consumed them only to count candidate sets and scores. Calling
the existing create-only artifact writer directly from `poll_field_observations` would retain the
values but could block PipeWire capture on catalog serialization, filesystem writes, sync, or
capacity failure. Reusing recording `source_pts_ms` for a live result would also state false source
provenance: the live owner has a monotonic capture interval, not recording PTS.

## Decision

`gamescope-result-recognition-gate` extends the existing field gate with a required create-only
`--recognition-artifact` directory. The older counts-oriented command remains available and does
not create this artifact.

The live gate moves each completed immutable registered observation to a dedicated artifact writer
through a capacity-two non-blocking queue. The writer, not the capture loop, performs catalog
serialization, observation writes, syncing, and manifest creation. Queue full, worker loss, write
failure, and bounded finish timeout are distinct compact outcomes. They do not alter the field or
resolver value and do not replace the capture/recognition result. The value-evidence gate exits
successfully only when every completed field observation was enqueued without a drop and the
artifact finished completely. It also requires at least one completed result observation containing
an accepted or typed-unknown `ResultSongResolution`; a music-select-only run is not live result
evidence. Artifact failure makes the value-evidence gate's top-level status and typed error agree
with its nonzero exit while the nested recognition counts remain intact.

One process-wide supervisor token covers worker creation through writer destruction. If bounded
finish times out, that worker may still complete an already-started write or publish its final
manifest; the token therefore prevents another live artifact worker from starting until the old
worker actually exits. The timed-out run remains failed regardless of later filesystem completion.

Recognition observation schema v2 represents timing as a tagged source:

- recording observations retain exact `source_pts_ms`; and
- live observations retain exact `monotonic_start_ms` and `monotonic_end_ms` from the bound field
  result.

Both sources otherwise call the same artifact serializer for exact OCR fields, the run-scoped
catalog table, complete candidate metrics, resolver decision/reason, and optional reviewed
expectation. Pixels remain in the independent diagnostic image store. Compact stdout retains only
execution counts, typed artifact status, and the manifest digest because it is an execution/control
surface; the local artifact is the evidence surface and deliberately retains the recognition
values.

## Consequences

- A live result can be reviewed using the same post-canonical values and serialization contract as
  recording simulation without filesystem latency entering the capture loop.
- A complete manifest is not sufficient if a producer-side queue drop occurred; the command checks
  the enqueue counts as well as writer completion.
- The existing `gamescope-field-observation-gate` retains its v1 schema and counts-only success
  condition. The new command emits `scorepeek-gamescope-result-recognition-gate-v1`.
- Artifact persistence can degrade independently while an already computed recognition result
  remains valid and observable.
- This gate supplies live recognition evidence. It does not establish event authority, release
  accuracy, target-host performance, or capture-profile support.
