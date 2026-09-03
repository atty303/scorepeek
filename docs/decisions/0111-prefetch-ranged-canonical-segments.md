# ADR 0111: Prefetch ranged canonical segments

## Status

Accepted

## Context

ADR 0110 overlaps whole-object segment downloads, but a single connection to the configured B2
S3-compatible provider reaches about 20 MiB/s. A 512 MiB object measured 20.31, 36.82, 69.23, and
112.50 aggregate MiB/s with one, two, four, and eight concurrent Range GETs. The active corpus also
showed that downloading only one four-range segment at a time leaves network latency visible after
the current decode consumes its prefetched file.

Two simultaneous four-range segment downloads failed one `segment_get` after eighteen completed
segments. The failure was typed `download_failed` after 906,964 microseconds, despite the
object-store client's lower-level retry policy. The provider response detail was intentionally not
recorded, so the exact cause is not established. A terminal transient failure therefore needs one
bounded application-level retry without weakening integrity failures.

## Decision

Replay prefetches up to two manifest-ordered segments ahead of the current decode and permits two
segment materializations process-wide. Each segment is split into four non-overlapping Range GETs
that write to fixed offsets in one anonymous temporary file. HEAD first enforces the declared total
size and supplies an ETag or version only as an object-stability condition, never as content
identity.

If an attempt ends in `download_failed` after the object-store client's retries, replay records a
retry event, waits 250 milliseconds, and retries the complete four-range materialization exactly
once. `not_found`, `permission_denied`, `object_changed`, `size_mismatch`, and `digest_mismatch` are
not retried. A second `download_failed` is final.

After all ranges finish, replay reads the assembled file from the beginning and verifies the exact
byte count and encoded SHA-256. Only the verified rewound file is passed to FFmpeg. Consumption
remains in manifest order, no persistent cache is created, and every pending download is joined and
released on success, failure, or cancellation.

## Consequences

- At most eight Range GETs are active for corpus segment data, organized as two independently
  verified four-range materializations.
- After the first segment, two-segment lookahead can overlap both future downloads with decode and
  recognition work.
- A recovered retry does not change the final `segment_get` status; the bounded local diagnostic
  records the handled `download_failed` as a retry event.
- One retry can repeat bytes already transferred by a failed materialization, but it occurs only
  after the lower-level object-store policy gives up and cannot create a persistent cache entry.
- Integrity and object-identity failures remain fail closed and are never hidden by retry.

This supersedes ADR 0110's whole-object GET and four-download replay scheduling decisions. ADR 0110
remains authoritative for direct final-key multipart publication and replay's complete verification
boundary.
