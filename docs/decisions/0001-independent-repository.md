# ADR 0001: Treat upstream as an external release input

- Status: Accepted
- Date: 2026-08-15

## Context

The former Linux branch edited Windows application entry points, capture code,
recognition code, and resource loading directly. Ongoing upstream changes made
that branch expensive to rebase and difficult to validate. The upstream
repository also has no public license grant suitable for copying its code into a
new public implementation.

## Decision

`scorepeek` uses an independent repository and history. It does not use a Git
fork relationship, submodule, subtree, merge, cherry-pick, or vendored upstream
source.

Upstream release tags are external inputs to a pinned, two-phase adoption
operation. Inspection records exact commits and hashes without unpickling.
Import accepts only resources matching a separately committed, human-approved
manifest, reports semantic changes, and must pass the private replay suite
before its generated pack can become active.

## Consequences

- Upstream updates cannot create source-level merge conflicts in scorepeek.
- Upstream schema drift becomes an explicit adapter/replay failure rather than a
  runtime surprise.
- Runtime algorithms and public APIs are owned entirely by scorepeek.
- The project remains private until upstream and game-asset redistribution
  rights are resolved.
