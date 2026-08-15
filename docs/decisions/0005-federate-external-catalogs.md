# ADR 0005: Federate external IIDX catalogs without fuzzy identity merging

- Status: Accepted
- Date: 2026-08-15

## Context

No one source provides universal IDs, current general-IIDX coverage,
INFINITAS availability, exact display variants, chart data, clear reuse terms,
and independent corroboration. Many apparent sources share MDB, Textage, or
official-page lineage.

## Decision

Use Tachi as the general-IIDX identity/chart anchor, Textage as an independent
metadata/display corroborator, and dqn/iidxapi as an official-page-derived
INFINITAS roster signal. Every observation preserves source revision, content
digest, parser version, field authority, and lineage.

Federation accepts exact existing bindings, policy-approved exact game IDs, or
exact title/artist/version plus strong chart agreement across independent
lineages. It never uses weighted majority or fuzzy title normalization for
identity. Ambiguous records are quarantined without replacing last-known-good
records.

The v1 public song ID is UUIDv5 over a fixed scorepeek namespace and the exact
Tachi song ID. Textage/dqn observations without a Tachi anchor remain
provisional; later bindings and display variants do not change an activated ID.

A dqn row has no stable key and contributes availability only when its exact NFC
title and artist resolve to one existing Tachi-anchored record. It never creates
or merges an identity. If a later snapshot omits any accepted tuple or no longer
resolves it to the same Tachi ID, all new bindings from that snapshot are
quarantined and the previous accepted set remains unchanged.

Each host fetches and builds its own content-addressed catalog. Raw or generated
third-party databases are not committed or redistributed. Scheduled and manual
sync share an exclusive writer lock from source acquisition through durable
activation. Activation revalidates its base digest, publishes fsynced staged
files with a same-filesystem rename, fsyncs the content-store destination
parent, then atomically replaces the fsynced active manifest and fsyncs its
separate parent directory.

## Consequences

- Safe new records and variants can activate automatically after source updates.
- Conflicts reduce coverage rather than creating a wrong merge.
- Source adapters, policies, and replay fixtures remain maintenance surfaces.
- Catalog additions replay stored CTC logits because they can change old
  runner-up margins without changing model weights.
- Concurrent or interrupted sync cannot silently discard a completed catalog
  update or expose a partially durable snapshot.
