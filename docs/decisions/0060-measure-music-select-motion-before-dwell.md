# ADR 0060: Measure music-select motion before adding dwell

- Status: Accepted
- Date: 2026-08-29
- Complements: ADR 0056's 10 Hz corpus boundary and ADR 0059's explicit deferral of
  music-select temporal state

## Context

Music-select changes more rapidly than the result screen. The active suite contains 2,556
music-select observations, but its bounded deduplicated canonical QOIs retain only 48 corresponding
frames and no adjacent pair within 250 ms. Those QOIs cannot establish whether a resolver decision
was made while the list was stopped, scrolling, changing selection, obscured by an option overlay,
or crossing a screen transition.

The active suite also binds one 10 Hz video-replay session to its original lossless FFV1 recording.
Visual review shows that full-frame difference is not a useful movement oracle: the central
background, rings, and light panels animate while a selected song is stationary. Conversely, the
right title list moves vertically during scrolling and remains visible through some overlays.
Central title and artist evidence can change on a different schedule from the right list.

Choosing a pixel threshold or using OCR agreement as a stationary label would make the future dwell
policy its own accuracy oracle. It could suppress real selection changes or install a transient
scrolling candidate without reviewed truth.

## Decision

Add a create-only offline command that reconstructs music-select review spans from one active-suite
10 Hz video-replay session:

```text
scorepeek-corpus music-select motion review-plan --store ROOT --session-sha256 SHA256 --video FILE --output FILE
```

The command verifies the active suite, session, observation object, capture-profile binding, full
video digest, packet dimensions, strictly increasing packet PTS, and absence of B-frames. It uses
ADR 0011 packet order to reproduce the production latest-frame sampling schedule. Review spans
include 500 ms before and after each contiguous music-select interval; overlapping padding is merged
so brief predicate flicker and transition frames remain visible.

Each sample is bound to the video digest, packet index, PTS, observation sequence, screen class,
capture profile, normalizer, and canonical layout. The production fractional-linear normalizer is
applied only to three canonical regions:

- the union of all twenty right-list title rows;
- the integrated active-list title row;
- the central selected-title region.

For each adjacent sample the draft records separate RGB L1, changed-pixel count, compared-pixel
count, and normalized L1 ppm for all three regions. It does not use whole-frame motion and does not
combine the regions into a score. ROI-only normalization is required to be pixel-identical to the
same part of full-frame production normalization.

The immutable `scorepeek-private-music-select-motion-review-draft-v1` starts every span as typed
`unknown` with reason `operator_review_required`. `stationary`, `scrolling`, and
`selection_change` are allowed operator labels, but this command never chooses one from motion,
OCR, screen classification, or candidate values. The draft is measurement and review material, not
accepted corpus truth, a dwell policy, an accuracy result, or event authority.

The video is a quiescent operator-selected input. It is opened once, hashed through that file
identity, and reopened for each child through its owned descriptor, so path replacement cannot mix
different files across probe or decode segments. Before publication, the path identity and digest
are checked again to detect ordinary replacement, append, or in-place-edit mistakes visible at that
boundary; this trusted-input check is not a filesystem lock or concurrent-mutation authority. The decoder seeks to verified keyframes, stops after
the bounded selected output count, and retains previous-region pixels for only the current span. A bounded,
concurrently drained decoder side channel reports every decoded frame PTS independently of the
packet inventory; decoded PTS, selected packet PTS, and retained observation timestamps must all
agree. The complete session `capture/run.json` must also bind the full video digest, profile,
normalizer, and current layout. Unsupported reordered video fails closed. Probe output, stderr,
packet count, timestamp range, review samples, decoder segment size, and child wall time are
bounded; every child is waited and timeout or abnormal processing terminates, reaps, and joins its
pipe readers. Output publication is atomic,
create-only, bounded to 16 MiB, and fsynced.

## Consequences

- Background animation no longer dominates the movement evidence used for operator review.
- Brief `unknown` screens, overlays, and selection transitions remain in context instead of being
  discarded as resolver noise.
- The active recording can produce reviewable temporal evidence without expanding the runtime or
  retaining another full-frame corpus.
- Music-select dwell remains unimplemented. A later decision must consume immutable reviewed spans,
  specify label application and evaluation, and demonstrate that stationary gating improves
  selection identity without hiding real changes.
