# ADR 0095: Parallelize independent field recognition and retain raw timing

- Status: Accepted
- Date: 2026-09-01
- Supersedes: ADR 0031 for one reused text session and ADR 0030 only for executing every field on
  one observer thread

## Context

The production field observer retained source-frame timestamps and busy skips but not the service
time of one field-bearing frame or its principal stages. Earlier target evidence therefore showed
roughly 500 ms between complete result observations without distinguishing serial text inference,
the fixed numeric batch, or catalog and resolver work. The observer also ran every general-text
field serially through one PP-OCRv6-small session whose ONNX Runtime intra- and inter-op counts were
both one.

Difficulty text and level recognition are independent observations. The former implementation
passed accepted difficulty into numeric preprocessing only to select a level cell layout before
inference. That ordering was an implementation dependency, not a recognition dependency; the
difficulty-to-layout decision belongs to the result join.

## Decision

- Keep one outer field-bearing-frame worker and the existing no-backlog admission contract. Within
  that frame, submit all five result PP-OCR jobs, or all four music-select PP-OCR jobs, immediately
  to a persistent pool of independent registered sessions. Run the one fixed-cell numeric batch on
  the outer worker concurrently with those text jobs.
- Each PP-OCR session retains CPUExecutionProvider, intra-op one, inter-op one, sequential graph
  execution, disabled arena, and the same model and decoder. Live pool width is half the available
  logical CPU count; offline replay reserves one logical CPU. Both policies have a minimum of one
  and a maximum of five, the current per-frame PP-OCR job count. Runtime manifest v3 binds these
  policies and records the selected policy, available parallelism, and actual worker count.
- Numeric preprocessing includes every registered level cell-layout variant in the same dynamic
  batch. After all recognizers finish, the deterministic join selects only variants matching the
  independently observed difficulty. Unknown difficulty or an unregistered layout leaves level
  unknown; no layout, field value, or failure is guessed.
- Join completed jobs in the existing field-definition and failure order. A frame remains atomic:
  any required recognition failure rejects the complete field observation, and completion order
  never changes artifact order, temporal state, or accepted event authority.
- Recognition observation v11 records per-frame raw microsecond durations without computing
  distributions or summaries: complete worker service time, PP-OCR parallel-stage wall time,
  numeric batch time when applicable, and post-recognition join time. Timing remains debug-only in
  the bounded local artifact and never enters stdout, the TUI, recognition decisions, or `/v1.sock`.
  Historical observation v5-v10 and runtime v1-v2 artifacts remain readable.

## Consequences

- A retained target run can distinguish long serial text work, numeric cost, and catalog/resolver
  cost before changing ONNX thread settings. Pool width and host-visible parallelism remain attached
  to every timed observation, so cross-host timings are not compared without their execution policy.
- Additional PP-OCR sessions increase startup time and resident memory. Game p99 frametime, RSS,
  frame timing, busy skips, and raw per-frame timing require a prospective target A/B before this
  path can satisfy the target performance or support gate.
- The active private suite replays all eight sessions, thirty-four episodes, 2,888 canonical frames,
  and one negative frame with unchanged recognition results. Development-host elapsed time is not a
  target performance result.
