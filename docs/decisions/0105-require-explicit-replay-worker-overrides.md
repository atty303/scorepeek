# ADR 0105: Require explicit replay worker overrides

## Status

Accepted

## Context

Corpus replay already exposes `--text-workers`, but a benchmark-only environment variable could
silently force option-free offline replay to one worker. That made the effective production default
depend on inherited process state and required developers to remember historical benchmark context.
The replay summary reported the resulting count, but only after the expensive run had started.

## Decision

Option-free live and offline recognition always select the CPU-derived production policy: live uses
half of available parallelism capped at twelve text workers, and offline uses available parallelism
minus four capped at twelve. Corpus replay worker overrides are accepted only through the public
`--text-workers N` option. A one-worker comparison therefore uses `--text-workers 1`; no environment
variable may alter worker selection.

The existing replay summary remains the observation authority for the actual selected text,
preparation, and decoder concurrency. The 2048 MiB memory default and explicit `--memory-mib`
override are unchanged.

## Consequences

Developers can run corpus replay without remembering a performance flag or cleaning inherited
benchmark state. Comparison commands become self-describing and reproducible. Tests continue to
cover the CPU-derived worker policy and additionally preserve the CLI distinction between an absent
worker override and an explicit value.

This supersedes ADR 0101 only for its internal single-worker environment configuration. Its
recording, canonical replay, semantic comparison, and performance-gate decisions remain in force.
