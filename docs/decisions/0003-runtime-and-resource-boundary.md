# ADR 0003: Use a Rust runtime and an isolated Python resource importer

- Status: Accepted
- Date: 2026-08-15

## Context

The upstream resources are gzip-compressed Python pickle/NumPy structures.
Importing them directly would make Python, arbitrary-code-capable unpickling,
upstream module side effects, and Windows-only dependencies part of the game
session runtime. Recognition also benefits from explicit types and exhaustive
failure handling.

## Decision

- Implement capture, recognition, temporal state, CLI, and the event service in
  Rust.
- Split adoption into inspection and import. Inspection never unpickles; it
  emits tag, commit, filename, and SHA-256 metadata for explicit human approval.
- Use Python only during import, inside a networkless restricted environment
  after every input matches a pre-existing approved manifest. A digest computed
  from the candidate being imported cannot approve that same candidate.
- Convert validated pickle structures into a deterministic, versioned,
  language-neutral pack consumed by Rust.
- Publish generated packs and model artifacts through a content-addressed store
  and an atomically replaced active manifest. The manifest binds input and
  output digests to schema, layout, recognition, capture, model, dictionary,
  and runtime compatibility identifiers.
- Use upstream templates for exact/unique matching and a pinned PP-OCRv6 small
  recognition model only as independent evidence for closed-catalog song names.
- Commit a separate approval record for every OCR model, dictionary, and config
  before download. It fixes immutable source revisions, exact digests,
  license/attribution data, preprocessing schema, and compatible OAR/ORT
  versions. Model synchronization never trusts an arbitrary local path.
- Disable OAR/model auto-download at build and runtime. The daemon receives only
  verified local content-addressed paths and has no model network fallback.
- Reject ambiguity instead of exposing a guessed value or generic confidence.

## Consequences

- Game sessions do not require Python or upstream code.
- Pickle is confined to a narrow, auditable trust boundary.
- Runtime and model dependencies must be pinned with hashes and license notices.
- Adoption needs a single-writer lock, staging cleanup, and replay gates before
  activation. Runtime starts `not_ready` on any manifest or artifact mismatch.
- Resource schema changes fail during adoption and require an explicit adapter
  version rather than permissive runtime compatibility code.
