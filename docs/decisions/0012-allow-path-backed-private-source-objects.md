# ADR 0012: Allow path-backed private source objects

- Status: accepted
- Date: 2026-08-18
- Supersedes: ADR 0010's requirement to copy every imported recording into the local corpus store

## Context

The first lossless recording is already a 14.8 GB local file. Copying it into a
temporary or workstation-local corpus does not improve its byte identity and
unnecessarily doubles local storage. S3 transport remains useful for later
replication, but configuring a remote is not a prerequisite for recognition
development.

Dataset generations must remain portable and content-addressed. An absolute
workstation path therefore cannot become part of a generation, source
manifest, recording manifest, or remote object identity.

## Decision

`recording import --external` admits a canonical absolute local path as a
private source locator. The locator is stored under the source SHA-256's local
content directory, mode 0600, but is excluded from the five dataset roles and
their generation digest. `SourceMedia` continues to mean the exact full-byte
SHA-256 and byte length regardless of whether the local resolver finds
`source.media` or `source.external.json`.

Every seal and verify operation resolves and hashes the complete source bytes.
Probe, extraction, and push open one source handle, hash that handle before
consumption, pass the same handle to the consumer, and hash it again before
publishing success. External files must remain regular, bounded, and not group-
or world-writable. A new locator is not published until media inspection and a
post-inspection hash succeed. If a file moves, reimporting the same bytes from a
new canonical path replaces only the local locator; the source, recording, and
generation identities do not change.

The copying import remains available. A remote pull into an empty store
materializes a private `source.media`; a pull into a store with an already
verified external object reuses its locator. A later push may stream the same
verified handle directly from that locator. It uploads to a unique owned remote
staging key, verifies both the still-open local source and all staged remote
bytes, conditionally publishes the immutable content-addressed key, and cleans
the staging key on every exit. Locator paths are never uploaded.

## Consequences

- A local generation is reproducible while each external locator resolves to
  the bound bytes. Missing or changed files fail closed.
- Copyless import avoids duplicating large recordings and preserves the same
  future S3 object identity.
- The operator owns durability and read-access policy for an external file;
  scorepeek does not chmod, move, or delete it.
- A generation manifest alone still does not contain a machine-local path.
