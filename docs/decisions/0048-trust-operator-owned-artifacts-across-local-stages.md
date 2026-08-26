# ADR 0048: Trust operator-owned artifacts across local stages

- Status: Accepted
- Date: 2026-08-26
- Supersedes: ADR 0012's unconditional post-consumption rehash of an operator-selected external
  recording; ADR 0047's mandatory transform-inspection checkpoint, duplicate problem-report tail,
  and per-invocation complete bundle verification
- Complements: ADR 0014's operator-owned local access policy and ADR 0043's bounded foreground
  failure evidence

## Context

Scorepeek's private workflows run inside one operator-controlled personal computing environment.
Several earlier decisions correctly bound external bytes, mutable activation, concurrent writers,
and destructive recovery, but then extended the same defensive posture across artifacts that
scorepeek had just created or the operator had explicitly selected. That produced repeated full
reads, cross-artifact re-adjudication, and new diagnostic machinery without an observed failure or
an independent correctness oracle.

Repeating the same deterministic normalizer over a scorepeek-written raw/canonical pair does not
independently establish transform correctness. Likewise, rehashing a multi-gigabyte local recording
before and after every consumer does not add useful evidence when no concurrent writer is part of
the operation. These costs delay the first target-machine use path and become permanent maintenance
surface.

## Decision

Operator-created or explicitly operator-selected local artifacts are trusted inputs. A downstream
local stage validates only the selected expected digest, required schema and fields, and invariants
needed to produce its requested result or catch an ordinary selection mistake. It does not
independently re-adjudicate every record, reconstruct an upstream decision, or repeat a complete
read solely to defend against deliberate same-operator substitution.

This trust does not remove validation at an actual mutable or external boundary. Network and remote
storage bytes, active catalog replacement, concurrent scorepeek writers, content-addressed
publication, filesystem traversal, crash recovery, destructive deletion, and runtime admission
continue to use their existing bounded integrity and lifecycle checks.

An operator-selected external recording is hashed once when an operation must establish its
declared byte identity. Probe, extraction, and push consume that same opened handle without an
unconditional second full local read after consumption. Explicit local verification still hashes
complete bytes. Remote push verifies the bytes that cross the remote boundary, and a future
operation with a real concurrent-writer contract must define its own bounded change detection.

Successive music-list and training stages consume canonical scorepeek-owned artifacts by digest.
They may validate shape, references, coverage, split isolation, and other invariants required by
their own result, but they do not rehash every upstream frame or crop, recompute every prior
measurement, reconstruct a prior review plan, or re-verify a complete upstream artifact when that
work has no new decision authority. Existing implementations that still do so are legacy behavior
to remove when those paths are next changed; the repeated checks confer no additional evidence.

The ADR 0047 target path starts with the portable private bundle rather than a transform inspector.
Raw-to-canonical comparison is added only after an observed transform mismatch, a normalizer
change, or another independent implementation creates a concrete comparison oracle. ADR 0043's
existing unknown tail, transition retention, and selected raw/canonical pairs remain the initial
ordinary problem evidence. A second pre-recognition tail, pending-report ledger, and worker
watermark are not added until retained target evidence demonstrates a specific gap.

A private bundle is completely verified when it is created, after transfer, and when it is
activated. Routine `scorepeek run --profile NAME` preflight selects the activated bundle/profile,
checks the manifest identity and host prerequisites, and lets the required resource loader verify
bytes as it reads them. It does not perform a separate complete bundle rehash before every capture
or read the same resource once for preflight and again for loading.

## Consequences

- The next executable checkpoint is the private operator bundle.
- Existing diagnostic retention is exercised on the 4K target before new retention machinery is
  designed.
- Local workflows retain digest-bound reproducibility while avoiding validation that cannot change
  a result or catch an ordinary mistake.
- Code that still implements the superseded repeated checks remains accurate legacy behavior, not
  a required security or evidence boundary, until simplified in a separately verified change.
