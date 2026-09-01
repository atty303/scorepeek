# scorepeek committed checkpoint

This file describes only the state included in the commit that contains it. Uncommitted changes are
outside the checkpoint; implementation history belongs in Git.

## Current milestone

- M3 common PipeWire receiver and Gamescope observed-frame profile: **in progress**.
- M4 canonical recognition, attempt resolution, and versioned event API: **in progress**.
- The runtime uses the fixed-cell numeric HOG/MLP model registered by private manifest v2. Text
  fields continue to use the registered PP-OCRv6-small bundle. Numeric model bytes, real crops,
  complete labels, and generated datasets remain outside the repository.
- `scorepeek-result-detected-v2` remains the accepted domain contract. Public `/v1.sock` authority,
  target support, push, release, and target installation are separate unverified boundaries.

## Implemented authority

- Every due 100 ms frame classifies screen state before field admission. Field-worker busy skips
  crop submission only; screen changes and attempt paths continue at screen cadence.
- Screen changes own monotonic episode IDs. MUSIC SELECT and RESULT field observations are adapted
  to bounded integer support over joint `(song, play type, difficulty)` catalog hypotheses. Raw
  candidate support accumulates as `u64`; summary normalization scales every candidate in an
  over-cap family by the same factor, preserving its margin. Empty observations do not erase prior
  evidence.
- MUSIC SELECT has current and challenger accumulators. RESULT has one accumulator per screen
  episode. MUSIC SELECT difficulty comes from five fixed canonical `PLAYER 01` marker slots; only
  central title, artist, and active-list title use PP-OCR. Difficulty narrows an already
  text-supported song and cannot generate identity. Level is positive advisory evidence only and
  never vetoes a candidate.
- The attempt reducer records selection-screen presence even without accepted identity. Sufficient
  result evidence may complete an observed select/play/result path; missing select/retry linkage or
  missing play remains non-authoritative. Confirmed attempt transition precedes the attempt's one
  domain result event. A MUSIC SELECT boundary clears prior select evidence and prevents retry
  inheritance; emitted attempt IDs remain deduplicated across transient screen-episode breaks.
- Current score and PGREAT/GREAT obey the score invariant. PGREAT/GREAT/GOOD/BAD are individually
  bounded by chart notes. Judgment totals, POOR, miss, FAST/SLOW, and combo break are not bounded by
  notes. Optional values remain typed and do not suppress an otherwise complete performance.
- Recognition artifact v13 retains raw fields, typed marker evidence, fixed-cell numeric evidence,
  joint catalog evidence, raw per-frame/stage microseconds, and completed/late field status.
  `resolver_state_changed` records only meaningful current/challenger/result/attempt transitions
  with raw and normalized family contributions. Bounded frame timing records measured screen
  resolver, attempt resolver, and synchronous output work exactly once for completed, busy-skip,
  not-applicable, failed, or late-episode paths. Readers continue to accept v5 through v12.
- Private regression label v4 can bind attempt and parent keys, select/play/result spans, and typed
  outcomes. Immutable v2/v3 labels remain readable; historical result-only sessions are not given
  inferred linkage.
- TTY output has exactly three vertical panes: Watcher (4 rows), Latest domain (9 rows), and Resolver
  (remaining rows). Latest domain reads only accepted v2 events. Resolver formats one typed snapshot
  with screen-local and attempt hierarchy. Private 10 Hz ticks update integer-second durations
  without adding run events, socket records, plain-output lines, or domain events. Rendering below
  80 by 25 is no-panic only.

## Verification boundary

- `mise run test` passes, including Rust runtime/corpus tests and the offline OCR tests.
- The saved target session `run-1788227723-404993416-858800-session-1` was used only as a read-only
  failure oracle. The fixed marker predicate resolves its final selection frames as attempt 1
  NORMAL and attempts 2--6 HYPER, matching operator truth. It was not imported or labeled.
- Active private generation `ecbc46bdfd428fbd337ec7de8af3c5d3c811b525a8f47aa7f6034f3fe1b887e1`
  replays 8 sessions, 34 episodes, 2,888 canonical frames, and one retained negative frame.
- The existing active private suite was not changed. The operator plans to replace it, so the new
  resolver policy still requires evaluation against a newly reviewed corpus with zero wrong joint
  acceptance and zero wrong domain events before replacing target authority.
- Target cadence, stage p50/p95/max, field busy skips, accepted attempts, one event per attempt, and
  event drop count have not yet been re-verified with this source checkpoint.

## Next executable task

Build the replacement reviewed corpus, implement the v4 attempt-policy evaluator and its `mise`
entry point, then run it and inspect wrong or unresolved joint outcomes before changing policy
constants. Target installation and prospective session validation remain a separate explicit
request.
