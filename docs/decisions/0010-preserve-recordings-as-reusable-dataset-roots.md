# ADR 0010: Preserve recordings as reusable dataset roots

- Status: Accepted
- Date: 2026-08-16

## Context

Collecting real game sessions is expensive. A recording made from game startup
through a sequence of plays and final shutdown must remain useful when layout,
normalizer, OCR, labels, or replay contracts change later. Treating extracted
frames or the current recognizer inputs as the dataset root would force a new
recording whenever downstream processing changes.

The private corpus is currently a durable local content-addressed store. It
needs an explicit, reusable S3-compatible transport without making object
storage available to the game-session runtime or allowing remote availability
to control local import.

## Decision

One complete recording is one opaque session and one immutable dataset root.
The original self-contained Matroska bytes are preserved without transcoding.
A high-level importer receives the finished recording and a versioned
capture-context document, probes the bytes with the pinned toolchain, derives a
capture profile from the context plus the observed media contract, and
publishes the source, profile, probe, and recording manifests to the local
private store. Recording, fixture, and session identities are derived from the
source SHA-256. Reimporting the same bytes and context is idempotent.

The user workflow is:

1. create one capture-context document for each fixed capture configuration;
2. start recording before launching the game;
3. play one or more complete sessions and exit the game;
4. stop recording and pass the finished Matroska file to `recording import`;
5. repeat only for missing semantic coverage or a new capture profile;
6. seal the imported recordings into an immutable dataset generation;
7. explicitly push that generation to a configured S3-compatible remote;
8. pull the same generation by digest wherever later processing is performed.

The CLI contract is:

```text
scorepeek-corpus recording import --store ROOT --capture-context CONTEXT RECORDING
scorepeek-corpus dataset seal --store ROOT DATASET_ID
scorepeek-corpus dataset push --store ROOT --remote REMOTE GENERATION_SHA256
scorepeek-corpus dataset pull --store ROOT --remote REMOTE GENERATION_SHA256
scorepeek-corpus dataset verify --store ROOT GENERATION_SHA256
scorepeek-corpus dataset remote-verify --store ROOT --remote REMOTE GENERATION_SHA256
```

Import and upload remain separate operations. Import never requires network
access and never uploads automatically. Remote mutation occurs only through an
explicit `dataset push`.

Remote storage uses immutable keys below a caller-owned prefix:

```text
v1/objects/sha256/ab/<sha256>
v1/generations/<generation-sha256>.json
```

A generation is a canonical manifest of typed object roles, byte sizes, and
SHA-256 digests. Objects are uploaded before the generation manifest. There is
no mutable `latest` pointer and no initial delete command. Pull and verification
always hash complete object bytes; S3 ETags are not content identities. Local
publication uses private staging, digest verification, no-clobber destination
creation, and fsync before success.

Verification parses all five typed roles as their canonical schemas and checks
their recording, source, profile, and probe references transitively. A digest
alone does not make arbitrary bytes valid for a typed role. Every local object
class and dataset-generation collection has per-object, count, and aggregate
byte limits. Pull computes all missing capacity under the corpus writer lock
before downloading an object and rechecks capacity at each publication; the
generation remains unpublished until all objects pass.
Role-specific document limits are checked from the generation before any GET.
Remote download staging uses an unlinked private temporary file, so a process
crash cannot leave an uncounted remote object in the store. Owned source and
document publication staging is recovered under the writer lock and its parent
directory is fsynced before capacity is evaluated.

The remote document contains only bucket, prefix, region, endpoint, addressing
style, and an explicit test-only loopback-HTTP permission. Credentials come
from the existing AWS provider environment,
prefer short-lived session or workload credentials, and never appear in CLI
arguments, repository files, structured output, or errors. Production
endpoints must be path-free HTTPS origins with no userinfo, query, or fragment,
and the bucket requires private access. HTTP is accepted only when
the document opts into an exact loopback IP endpoint for the focused local
test; hostnames and remote HTTP endpoints remain invalid. Operators must configure a
bucket lifecycle rule for abandoned multipart uploads because a process crash
cannot always abort them.

The remote transport is implemented only in the offline `scorepeek-corpus`
crate. Later normalizer, layout, crop, label, model, and replay generations
reference the preserved recording generation and can be rebuilt without
recording again. A new recording is required only for missing screen/session
coverage or a changed observed capture contract.

The implementation uses `object_store` 0.14.1 with only its AWS feature, Tokio
1.53.1, and the already-transitive `futures-util` 0.3.34 streaming interface for
bounded asynchronous file transfer. The already-transitive `url` 2.5.8 parser
enforces the endpoint-origin contract. They are isolated to the offline corpus
crate; the game-session runtime dependency graph is unchanged. The focused CLI
round-trip uses mise-pinned `rclone` 1.74.2 as a test-only S3-compatible server;
it does not enter either Rust binary or the game-session runtime.

## Consequences

- Original recordings, not current extracted frames, are the long-lived
  reproducibility boundary.
- Dataset reuse is addressed by immutable generation digest rather than a
  bucket-local name or mutable pointer.
- Recording import remains usable while the remote is unavailable.
- S3 support increases the offline corpus dependency graph and binary size but
  does not enter the game-session runtime.
- Remote storage preserves private data and therefore requires private access,
  TLS, bounded transfers, and secret-safe failures.
- Derived artifacts can evolve without changing the recording-generation
  contract or requiring another game session.
