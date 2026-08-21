# Play-attempt diagnostic contract v1

This document fixes the first replay boundary selected by ADR 0023. It is a
diagnostic scenario contract, not the accepted live state machine, capture
support, or a public event API change.

## Timeline proposal and operator review

scorepeek first infers a timeline from recording evidence alone. The output is
always a **proposal**: `proposed_episodes` identifies screen-local ranges and
`proposed_attempts` links a music-selection episode through gameplay to result.
Every observed screen kind, including `other`, remains visible as an episode;
`other` breaks attempt linkage rather than disappearing from the composition.
An attempt proposal contains only those three episode links. Live
`event_outcome` never changes the proposal oracle; it is used only for miss
accounting and discrepancies in the review report.
The initial `timeline_review` is `needs_operator_review` with
`operator_notes_applied: false`.

Each sampled canonical frame carries `timeline_evidence`, which is the
recording-replay inference used by the pure reducer. It is separate from
`screen_observation`, `song_decision`, and `event_outcome`, which preserve what
the live path did at that point. A replay-inferred result may therefore be
reported even when the live detector was not run.

The v1 synthetic replay contract requires at least two fresh music-selection
observations spanning at least 250 ms before that episode can seed an attempt.
Its policy is fixed at a 250 ms target interval, a 500 ms maximum observation
gap, and the synthetic 1,000 ms result dwell value.
`calibrated_profile` evidence is rejected until a separate versioned contract
can bind an immutable calibration artifact. These values make stability
explicit for this scenario; they are not a target-profile cadence or support
claim. An unstable selection remains in the
episode report but is not linked.
Each music-selection observation also carries a recording-derived fingerprint
of the versioned selection-context pixels. Only the trailing run of one
fingerprint can satisfy stability; a change or unknown continuity resets the
candidate without consulting the live `song_decision`. Fingerprints and changes
are shown in the operator report.

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
result episode is counted independently of whether it can be linked to an
attempt. Its event absence is counted only when a complete segment's measured
maximum observation gap is strictly below v1's fixed synthetic 1,000 ms result
dwell. This is diagnostic miss accounting only, not target-profile calibration;
partial and dropped intervals never prove absence.
Because a segment-level partial/dropped marker does not locate every hidden
transition, the reducer disables all attempt linkage in that segment. Explicit
unknown evidence breaks episodes at its exact sequence, while its segment-local
sequence and complete live detector/song/event outcome remain visible in the
gaps/discrepancies section. Covered observations retain the same typed
per-observation live evidence, including IDs and reasons, rather than only
aggregate counts. A recording may validly
produce zero episodes or zero attempts; its gaps and review questions must still
be reportable.

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
Opaque accepted song IDs are listed in the local review report so conflicts
within an episode or between linked selection/result episodes cannot disappear
behind aggregate counts.
For result miss accounting, `suppressed` and `absent` both mean that no public
result event was emitted; suppression remains separately visible with its typed
reason. Accepted songs or emitted events on gameplay/other screens and duplicate
result emissions are retained as input evidence and called out as discrepancies
rather than making the report unreadable.

## Next implementation boundary

The pure deterministic reducer and report renderer consume this fixture and
verify that the committed proposal is reproduced from `timeline_evidence`. The
public scenario validator performs the same oracle check, so structural
validation cannot accept stale links on a partial or dropped segment. The
next slice must supply recording-replay screen evidence from canonical frames
without using the live event path as its trigger. Live recording, event
delivery, automatic confirmation, and target cadence calibration remain
outside that slice.
