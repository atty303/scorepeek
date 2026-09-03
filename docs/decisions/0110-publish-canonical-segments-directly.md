# ADR 0110: Publish canonical segments directly

## Status

Accepted

## Context

ADR 0109 published a canonical segment through a unique staging object, a complete staging GET,
conditional server-side copy, and a complete final GET. That made client traffic roughly three
times the segment size and tied import latency and compatibility to provider-specific copy
behavior. Canonical object identity is already the declared full-byte SHA-256, and the local source
remains open while import reads it.

The configured S3-compatible provider also measures about 20 MiB/s for one GET but scales nearly
linearly through four concurrent GETs. Segment-at-a-time download followed by decode therefore
made remote replay wait on network latency that could run concurrently with current-segment work.

## Decision

Import starts a multipart upload at the final frame-corpus SHA-256 key. While filling fixed-size
parts it computes the complete local byte count and SHA-256. A mismatch aborts the multipart upload
before completion. Every part uses the object-store client's signed SHA-256 payload; a provider
checksum rejection is an upload failure. Successful multipart completion is the import publication
boundary. Import performs no staging-object PUT, upload readback GET, server-side copy, or final
readback GET.

An existing final object is reused after HEAD only when its byte length equals the declaration; a
different length fails closed. Writers in one process serialize work by digest. Cross-process
create-only publication is not required: concurrent normal writers have independently verified the
same canonical digest before completion and may publish identical bytes to the same key. ETag is
not an identity.

Replay keeps ADR 0109's verification boundary. A remote-only segment is completely GET into an
anonymous temporary file while enforcing the declared maximum length and encoded SHA-256, and only
the verified rewound file is passed to FFmpeg. Replay prefetches up to four manifest-ordered
segments per active session and permits at most four concurrent remote downloads process-wide.
Consumption remains in manifest order, no persistent cache is created, and every pending download
is joined and released on success, failure, or cancellation.

The bucket still requires an `AbortIncompleteMultipartUpload` lifecycle rule for uploads whose
process or host disappears before an in-process abort can run.

## Consequences

- Import client traffic is one PUT stream instead of a PUT and two full GET streams.
- Import no longer depends on Copy Object support, timeout behavior, or conditional-copy semantics.
- HEAD reuse proves declared size and availability, not content; content identity for a new upload
  comes from the local pre-completion digest and signed part payloads.
- Replay retains complete remote size and SHA-256 verification before decode while overlapping
  network and recognition work.
- A remote implementation that accepts corrupted signed payloads violates the configured S3 trust
  boundary; scorepeek does not compensate with an import readback.

This supersedes ADR 0109's remote staging, create-only copy publication, upload readbacks, staging
recovery, and serial segment materialization decisions. ADR 0109 remains authoritative for the
environment-only configuration, metadata placement, replay integrity, diagnostics, and absence of
corpus storage quotas.
