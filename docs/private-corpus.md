# Private canonical session corpus

The attempt regression corpus replays complete, operator-reviewed recording sessions. Its pixel
authority is the canonical RGB8 1920x1080 stream produced during a `scorepeek run --record`
session. It does not ingest ordinary video, invoke a capture normalizer, expand segments into QOI
objects, or deduplicate frames by pixel content.

## Recording boundary

`scorepeek run` performs production recognition without saving artifacts. `scorepeek run --record`
starts capture diagnostics, recognition observation v17, run-event v6, the canonical session
recorder, and joined diagnostic session v5 together. `--profile NAME` may appear before or after
`--record`.

Recording preflight requires bounded store capacity and a PATH-resolved FFmpeg that exposes
`libx264rgb`. The artifact records the executable digest and first version line. Recording queue
loss, encoder failure, publication failure, or shutdown timeout marks the recording partial but
does not change screen resolution, attempt finalization, or domain event emission.

The canonical recorder indexes every 10 Hz due tick with original sequence, monotonic time, raw
screen, active semantic episode ID, and either `retained` or a typed intentional-elision reason.
It retains every `MusicSelect`, `DecideTransition`, and `Result` frame. It retains the session's
first and last ten ticks and ten-tick windows around all raw-screen changes, including entry to and
exit from `Unknown`. Only stable `Play`, `ModeSelect`, and `Unknown` interiors are elided.

Contiguous retained frames are lossless RGB Matroska segments. Gaps, chronology resets, 600 frames,
or session end close a segment. Complete segments have matching input and decoded RGB24 digests and
frame counts.

## Import and review

Verify and import one complete joined session:

```text
scorepeek-corpus diagnostic verify /absolute/recorded-session
scorepeek-corpus corpus import-diagnostic --store /absolute/private-corpus-v2 --diagnostic /absolute/recorded-session --review-draft /absolute/review.json
```

Import publishes the diagnostic components as immutable digest-addressed objects. The review draft
lists retained sequence identities; it does not create a separate image object per tick.

Regression truth uses only `scorepeek-private-session-regression-label-v5`. Each episode includes:

- a label-local `attempt_key` and optional earlier `parent_attempt_key`;
- ordered select, decide, play, and result sequence spans;
- an `accepted`, `abandoned`, `unlinked`, or `no_result` outcome;
- song/chart identity, clear type, numeric performance, and an explicit ordered distinct
  `play_options` list, including `[]` when no option was shown.

Every span endpoint must be retained on its expected raw screen. Select, decide, play, and result
spans must be ordered. Every tick inside `DecideTransition` and `Result` spans must be present,
retained, and classified as that screen. Attempt keys are unique and a parent must name an earlier
attempt in the same label.

Apply the reviewed truth create-only:

```text
scorepeek-corpus review apply --store /absolute/private-corpus-v2 --draft /absolute/review.json --labels /absolute/operator-labels-v5.json
```

Partial sessions cannot become active regression entries. There is no legacy label reader,
converter, or archive path.

## Replay semantics

```text
scorepeek-corpus corpus replay --store /absolute/private-corpus-v2
```

Replay losslessly decodes retained segment frames and supplies their original sequence and
monotonic time to the production screen-episode, field-recognition, attempt, RESULT-finalization,
and run-event reducers. Frames are streamed one at a time. Intentional gaps are not filled with
synthetic pixels: PLAY and MODE SELECT gaps continue their semantic screen, while a retained
UNKNOWN suspends until the next retained known frame or session end. A DecideTransition gap is an
invalid suite.

OCR may complete out of order but field evidence is committed by admission sequence. Offline
capacity is twice the selected text-worker count and the producer waits rather than dropping a
frame. Replay stdout reports `text_workers`, summed `text_batch_wall_us`, summed ordered
`field_frame_wall_us`, and `corpus_wall_us`. The internal comparison run sets
`SCOREPEEK_INTERNAL_SINGLE_TEXT_WORKER=1`; ordinary offline replay follows the
available-parallelism policy.

For every accepted label, replay requires exactly one ordered
`scorepeek-result-detected-v2` event with equal semantic payload, ordered play options, and normalized
parent relation. The runtime session ID, runtime attempt IDs, emission tick, and diagnostic metadata
are not truth. Missing, duplicate, extra, payload-different, play-option-order-different, and
parent-different events fail replay. Non-accepted outcomes require no event.

## Normalization verification

Observed-to-canonical correctness is independent of attempt regression. Verify it from a
profile-calibration artifact or an explicitly bound observed/canonical pair. `corpus replay` never
starts the normalizer; FFmpeg is used only to decode the already-canonical lossless segments.

## Promotion boundary

The old private corpus remains untouched until a fresh v5 session can be recorded, verified,
imported, reviewed, and replayed from a temporary root. One-worker and default-pool runs must emit
identical domain events. OCR wall time and whole-corpus wall time must both improve before the pool
is called a speedup. Target install, numeric-manifest activation, public socket authority, push,
release, and deletion of the old corpus are separate verified boundaries.
