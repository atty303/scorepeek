# Play-attempt diagnostic contract v1

This document fixes the first replay boundary selected by ADR 0023. It is a
diagnostic scenario contract, not the accepted live state machine, capture
support, or a public event API change.

## Timeline proposal and operator review

scorepeek first infers a timeline from recording evidence alone. The output is
always a **proposal**: `proposed_episodes` identifies screen-local ranges and
`proposed_attempts` links a music-selection episode through gameplay to result.
The initial `timeline_review` is `needs_operator_review` with
`operator_notes_applied: false`.

Before treating a scenario as confirmed, report this composition to the
operator:

1. recording/scenario identity and every binding-set digest;
2. segment ranges, sampling gaps, and `complete`/`partial`/`dropped` status;
3. inferred episode kind, sequence range, and transition boundaries;
4. proposed selection → gameplay → result links;
5. detector, song-decision, and emitted/suppressed/absent event discrepancies;
6. uncovered ranges, ambiguous boundaries, and questions about recording-external exceptions.

The report is the review surface for information that is absent from the
recording and known only to the operator. Such information must not be guessed
or silently folded into recording evidence. A later confirmed scenario must
set `operator_notes_applied: true`; the original recording-derived proposal
remains independently reproducible.

## Result, observation, and review separation

Each observation records monotonic sequence/time, a canonical-frame artifact,
screen observation, song decision, and event outcome. Independently proposed
episode ranges let replay enumerate a possible result even when screen
detection was not run and no result event was emitted. `unknown`, `rejected`,
`not_run`, `suppressed`, and completeness reasons carry no recognized field
values.

The synthetic scenario proves only schema, ordering, and missed-event
enumeration. Its 1,000 ms result dwell is not target-machine calibration. A
profile can count result absence only when a complete segment's measured
maximum observation gap is strictly below a separately calibrated minimum
result dwell. Partial and dropped intervals never prove absence.

## Bounds and privacy

The v1 parser rejects documents above 1 MiB and bounds a scenario to 8
segments, 4,096 observations per segment, 1,024 episodes, and 512 attempts.
A segment is at most 15 minutes and 32 GiB of canonical artifacts. Local
retention is bounded to 2 normal runs, 6 priority runs, and 128 GiB total.
Recording is enabled by default but supports operator opt-out. Full canonical
RGB8 frames are retained only until the ROI contract stabilizes; remote export
is disabled.

Artifacts are referenced by kind, byte length, and SHA-256 rather than path.
The contract contains no raw OCR candidates, player/rival fields, credentials,
or recognized display strings. It does not change the public NDJSON boundary.

## Next implementation boundary

The next slice may implement a pure deterministic reducer that consumes this
fixture and emits the proposed timeline report. Live recording, event delivery,
automatic confirmation, and target cadence calibration remain outside that
slice.
