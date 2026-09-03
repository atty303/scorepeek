# ADR 0109: Store canonical corpus segments in S3

## Status

Accepted

## Context

Attempt-corpus sessions retain lossless Matroska segments whose aggregate size dominates the local
corpus. Their session, label, suite, tick-index, and binding documents are small and remain useful
for local inspection. Copying every segment into `objects/` makes corpus growth consume workstation
disk even though segment identity is already a full-byte SHA-256.

The former recording-dataset S3 transport was superseded as a capture-regression entrypoint. Its
content-addressed staging, conditional publication, and full-byte verification properties remain
applicable, but its dataset generations, local locator files, pull-to-store workflow, and aggregate
store-capacity accounting do not.

## Decision

When `SCOREPEEK_CORPUS_S3_URL` and `SCOREPEEK_CORPUS_S3_REGION` are present, diagnostic import
uploads only canonical Matroska segments to the configured private S3-compatible store. The URL is
`s3://bucket/optional/prefix`; optional `SCOREPEEK_CORPUS_S3_ENDPOINT` must be an HTTPS origin and
`SCOREPEEK_CORPUS_S3_PATH_STYLE` accepts only `true` or `false`. Credentials come only from the
standard AWS process environment: a complete static-key pair, web-identity pair, task-relative
container URI, or full container URI plus authorization-token file. Partial or empty credential
sets fail before client construction and cannot fall back to instance metadata. No remote config
or locator file is accepted or persisted.

Remote objects use their encoded SHA-256 below a frame-corpus-specific namespace. Import uploads to
a unique staging key, reads and hashes every staged byte, publishes create-only, reads and hashes
the final object, and cleans staging before publishing session identity or review documents. ETag
is never identity. A later remote operation removes only scorepeek-owned staging keys older than
seven days, so interrupted publications are reclaimed without touching a concurrent writer; a
local-only consumer performs no remote request.
Non-segment artifacts remain local. Without remote environment configuration, import remains
local-only.

Because an abrupt process or host termination cannot abort an in-flight S3 multipart upload, the
operator must configure the target bucket with an `AbortIncompleteMultipartUpload` lifecycle rule.
The rule is an object-store prerequisite rather than scorepeek configuration; scorepeek still
aborts every multipart upload whose failure it observes in-process.

Consumers prefer an existing verified local segment. If it is absent, they GET the exact remote
digest into an anonymous mode-0600 temporary file, enforce the manifest's per-object byte contract,
hash the complete encoded object, rewind it, and only then give it to FFmpeg through stdin. The
file is released with the decode operation and is never cached. A missing local segment without
remote configuration fails closed.

Corpus stores have no aggregate object-count or byte-capacity policy. This applies to the local
corpus, remote corpus objects, and concurrent temporary segment materialization. Per-object and
per-document bounds remain input and allocation safety contracts, not storage quotas. Replay's
memory account remains independent because it bounds live decoded-frame and recognition state.

Remote publish and GET operations record bounded local operation diagnostics containing only the
operation, status, typed error, object digest, declared bytes, and duration. Credentials, endpoint,
bucket, prefix, headers, and provider response bodies are excluded. Remote diagnostic failure does
not change corpus behavior and remote export is not added.

## Consequences

- Local steady-state corpus size is metadata plus any deliberately retained local segments.
- Replay and numeric authoring require network access when a referenced segment is remote-only.
- Import source diagnostics are not deleted automatically.
- Existing local segments remain valid and can be uploaded and removed in a separately approved,
  disposable migration without rewriting session, label, or suite identities.
- The target bucket must expire incomplete multipart uploads independently of scorepeek process
  lifetime.
- Replay summary v4 distinguishes local decodes from remote downloads; diagnostic import summary
  v3 reports remote transfer and reuse.

This supersedes ADR 0101 only for requiring imported canonical segments to be local immutable
objects, and supersedes the aggregate corpus-storage capacity requirements retained from ADR 0010.
