# ADR 0069: Evict the oldest inactive diagnostic run at capacity

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0025 only for capacity eviction eligibility

## Context

The diagnostic store distinguished normal and priority retention periods, and also excluded every
unexpired priority run from capacity eviction. A target store containing only partial runs therefore
reached its aggregate byte limit and refused a new ordinary run. The newer observation was more
valuable for diagnosing the current executable than retaining every older partial run.

Age retention and capacity admission serve different purposes. Priority remains useful for keeping
failure evidence longer while space exists, but it must not make the bounded store permanently
unable to admit fresh evidence.

## Decision

Keep the 24-hour normal and seven-day priority expiry periods. After removing expired runs, admit a
new publication by evicting the oldest inactive run first, regardless of its priority classification,
until the exact requested bytes fit. Continue to prove that enough inactive bytes are reclaimable
before the first capacity deletion. Never evict the run protected by the active exclusive lease.

Freeze remains an age-retention operation, not a capacity pin. Operators who need durable evidence
beyond the bounded live store must export it.

## Consequences

- A full store can admit a new run whenever enough inactive managed data exists.
- Error, timeout, crash, partial, and frozen runs still receive the longer expiry period while space
  permits, but can be removed under capacity pressure.
- Capacity reclamation remains deterministic by retention time and run ID.

