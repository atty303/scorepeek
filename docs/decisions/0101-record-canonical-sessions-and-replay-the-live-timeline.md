# ADR 0101: Record canonical sessions and replay the live timeline

## Status

Accepted

## Context

The QOI-first private corpus retained selected pixels but not the complete raw-screen chronology
needed to reconstruct semantic suspension, selection handoff, attempt linkage, RESULT close-time
drain, and final event emission. Its video authoring path also mixed observed-to-canonical
normalization with the recognition regression oracle. Independently, the text runtime submitted the
fields of one frame in parallel but joined that frame before admitting the next one, leaving the
registered single-threaded ONNX sessions underutilized.

Routine execution still needs a low-overhead mode. Recording failure must not become recognition
authority or change domain events.

## Decision

Routine recording is opt-in through `scorepeek run --record`. Without it, scorepeek creates no
diagnostic, recognition, run-event, joined-session, or canonical-video artifact. With it, all five
components belong to one session and are subject to bounded preflight and retention. The removed
`--no-recording` and `--record-attempts` spellings are unknown options.

The canonical component records fixed RGB24 1920x1080 frames at the existing 10 Hz due ticks.
`MusicSelect`, `DecideTransition`, and `Result` are retained completely. Stable interiors of
`Play`, `ModeSelect`, and `Unknown` may be intentionally elided, while the first and last ten ticks
and ten-tick windows around every raw-screen transition are retained. Every observed tick remains
in a typed index. Intentional elision is distinct from queue loss, encoder failure, and shutdown
timeout.

Consecutive retained frames are written through an external PATH-resolved FFmpeg child as lossless
`libx264rgb` Matroska segments with a one-second GOP. A gap, chronology reset, 600 frames, or session
end closes a segment. Scorepeek records the FFmpeg executable digest and version, bounds stderr,
closes stdin, waits at most 30 seconds, and always reaps the child. It decodes every finished segment
back to RGB24 and verifies both frame count and the input-pixel digest before complete publication.
Recorder degradation marks only recording completeness partial.

Joined diagnostic sessions advance to v5, recognition observations to v17, and the registered
PP-OCR runtime manifest to v4. Recognition retains raw per-frame stage durations rather than
aggregate statistics. The PP-OCR field constant is named `MAX_TEXT_FIELDS_PER_FRAME`; it is a
layout limit, not a worker limit. Each ONNX session remains one intra/inter-op thread. Live uses
half of available parallelism with a minimum of one worker; offline uses all but one. Text dispatch
uses one pool-wide round-robin cursor. The outer field coordinator may admit two live frames, or
twice the offline text-worker count, and always commits results in admission sequence.

The private attempt corpus clean-cuts to complete v5 joined sessions. Import stores the canonical
segments, tick index, binding, recognition, and event components as immutable objects without QOI
expansion or pixel-content deduplication. Replay decodes only retained canonical frames and never
runs a normalizer or synthesizes intentional gaps. Label v5 is the sole regression truth and binds
ordered attempt spans, local attempt and parent keys, outcome, semantic payload, and ordered distinct
play options. `DecideTransition` and `Result` spans cannot contain missing or intentionally elided
ticks. Accepted attempts require exactly one ordered, payload-equal v2 result event; every other
outcome requires none.

Observed-to-canonical verification is no longer an attempt-corpus operation. It requires a separate
profile-calibration or explicit observed/canonical-pair workflow.

## Consequences

Long PLAY interiors no longer dominate corpus storage, while screen transitions and every evidence
bearing screen remain replayable. Corpus runtime no longer pays an FFmpeg normalization process per
QOI. Complete publication is stricter because any retained-frame loss or lossless-decode mismatch
prevents suite activation.

Recording requires an external FFmpeg with `libx264rgb` and sufficient bounded store capacity.
Ordinary unrecorded recognition has no artifact evidence by design. Worker-pool speedup remains a
measured claim: one-worker and default-pool replay must emit identical events, and both OCR wall time
and whole-suite wall time must improve before target-performance authority changes.
The one-worker comparison is selected only by the internal
`SCOREPEEK_INTERNAL_SINGLE_TEXT_WORKER=1` replay configuration; replay reports its selected worker
count and raw text/frame/corpus wall durations.

This supersedes ADR 0056 for the attempt-corpus boundary, ADR 0075 for routine recording policy,
ADR 0082 for attempt regression replay, and ADR 0095 for the outer frame scheduler. Their historical
measurements remain valid in their original scope.
