# Diagnostic store controls

Diagnostic controls operate on one existing diagnostic-run root. They do not
run recognition or infer an INFINITAS session. Retention is application-owned
and runs automatically under the store writer lease before and during a new
diagnostic run.

```text
mise run diagnostic:status -- --root /absolute/existing/diagnostic-root
mise run diagnostic:list -- --root /absolute/existing/diagnostic-root
mise run diagnostic:freeze -- --root /absolute/existing/diagnostic-root --run-id RUN_ID --run-sha256 RUN_SHA256 --manifest-sha256 MANIFEST_SHA256_OR_NONE
mise run diagnostic:delete -- --root /absolute/existing/diagnostic-root --run-id RUN_ID --run-sha256 RUN_SHA256 --manifest-sha256 MANIFEST_SHA256_OR_NONE
mise run diagnostic:export -- --root /absolute/existing/diagnostic-root --run-id RUN_ID --run-sha256 RUN_SHA256 --manifest-sha256 MANIFEST_SHA256 --destination /absolute/nonexistent-directory
```

`status` schema v2 reports the fixed local policy, whether an exclusive writer
lease is currently held, actual managed bytes, remaining bytes
under the 8-GiB aggregate budget, and aggregate completeness/priority counts.
`list` schema v2 reports only opaque run IDs, the exact `run.json` SHA-256, an optional
completion-manifest SHA-256, terminal status/completeness, priority, and managed
bytes, including whether priority came from an operator freeze. It does not expose paths, pixels, OCR text, song/player values, replay
request fields, or recognition bindings.

A canonical `run.json` without `manifest.json` is listed as `partial`, with no
terminal status or manifest digest, and is priority evidence. This represents
the observable state at inspection time; it does not decide whether a worker is
currently active or a prior process crashed. A completion manifest is accepted
only when it strictly parses, binds the exact start document, and its manifest
and total byte accounting match the directory snapshot. Typed frame, fact,
degradation, and reason-count entries must preserve the writer's bounds and
outcome semantics, and their declared filenames and bytes must cover the exact
regular-file set. A partial run may contain only `run.json` and bounded
writer-named frame/fact files within the writer's per-type count and fact-size
bounds. Producer package version must be valid SemVer and is recorded identity;
the v1 schema, not equality with the inspecting binary version, determines
compatibility.

One durable zero-byte inventory marker is the only non-run store-root entry
accepted by the inventory. The writer locks both the store-root directory inode
and one canonical-root-path-derived, zero-byte ownership anchor in its stable parent for the
entire run; status takes the same locks in shared mode while taking an idle
snapshot. A legacy root without the parent anchor remains read-only under its
root lock, while the first writer durably creates the anchor. The root marker is
an inventory sentinel, not the lease identity. Scorepeek resolves aliases and
intermediate symlinks and revalidates both the requested path and canonical root
against the locked inode before and after anchor acquisition. Scorepeek processes
therefore derive and honor the same parent anchor, so cooperative cross-process writers serialize
even if the root pathname is rebound while a writer holds the old root inode.
This prevents scorepeek retention from deleting an active scorepeek run. The
lease is advisory; deliberate same-UID replacement of both the root and its
parent anchor is outside the operator-trusted artifact boundary.

Retention uses completion-manifest publication time for completed runs and the
last run-directory entry creation time for start-only partial runs. It removes
normal runs after 24 hours and priority error, timeout, or partial runs after
seven days. When additional bytes would cross the 8-GiB aggregate limit, it
removes the oldest non-priority normal runs until the exact publication fits.
Before deleting anything for capacity, it proves that all eligible normal bytes
can make the exact publication fit; an impossible request leaves them intact.
If only active or unexpired priority evidence remains, the new diagnostic write
returns a typed capacity degradation without changing recognition results.
Reservations are released when frame, fact, or final-manifest publication does
not commit, so later capacity accounting remains equal to published bytes.
Deletion first atomically renames a strictly inspected run into a reserved
staging name, durably publishes an ownership marker through a fixed recoverable
marker-publication state bound to that run ID and its exact pre-delete
filename/byte inventory, and then removes only a remaining
subset of that inventory. Unknown entries or replacements observed during
preflight fail before payload unlink. Payload removal is fsynced while the marker remains, then marker removal
is fsynced as an empty tombstone before the directory and root are removed. The
next writer recovers only a valid intact pre-marker run or a marker-bound
scorepeek-owned deletion staging directory. An empty reserved tombstone left
after marker removal is safe to finish removing; other reserved or malformed
entries are preserved and fail closed.
The inventory does not claim protection against a non-cooperative same-UID
process racing a pathname replacement after its final identity check.

Inspection is bounded to 8,192 run directories, 50,000 regular files per run,
64 KiB for `run.json`, and 16 MiB for `manifest.json`. Run IDs and schemas are
strict. Symlinks, nested directories, unmanaged root entries, over-bounds
documents, byte-accounting mismatches, and a root or run directory that changes
during any part of inspection fail the whole command with a value-free error.
Per-run or aggregate policy overflow also fails instead of returning zero
remaining capacity as healthy state. The controls
do not rehash every QOI/fact artifact; strict replay or a future explicit verify
control remains the integrity boundary for artifact contents.

`freeze` and `delete` require the current run digest and exact manifest digest.
For a partial run with no manifest the explicit manifest confirmation value is
`none`; supplying `none` for a complete run or a digest for a partial run fails
without mutation. Freeze is idempotent and publishes a canonical marker inside
the run. It changes retention priority and restarts the seven-day priority age
at marker publication without changing artifact byte accounting. A fixed
freeze-publication staging name makes interruption recoverable by the next
writer or mutating control. Only a regular non-symlink staging file can be
discarded as an incomplete owned publication; other reserved entries are
preserved and fail closed. Delete uses the same rename-first recovery path as
retention and removes frozen metadata with the run.

Local export accepts complete runs only. It takes the store lease, revalidates
the supplied digests, rehashes every manifest-bound artifact, independently
hashes the optional validated freeze marker, copies regular files with
create-only mode, and publishes canonical
`export.json` as the last fallible commit point, after artifact and directory
durability preparation, through the same atomic create-only publication primitive used
for private manifests. The destination must be an absolute nonexistent directory
whose resolved existing parent is outside the canonical store root; lexical aliases
and intermediate symlinks cannot route it back into the store. Remote export remains disabled. A failure can leave the
claimed destination as an explicitly incomplete export without `export.json`;
scorepeek never overwrites or silently cleans that destination, so the operator
must select a new destination or remove the incomplete one explicitly. Export
JSON results expose only opaque identities, digests, counts, and bytes—not the
destination path or artifact contents.

## Recording completeness

Under [ADR 0116](decisions/0116-limit-recording-completeness-to-runtime-loss.md), completeness
describes runtime persistence loss: queue/capacity limits, writer or encoder failure, unavailable
workers, abandoned admitted work, flush timeouts, and interrupted publication. It does not certify
recognition accuracy or input validity. An operation may fail while its diagnostics are complete.
Internal typed facts are not independently schema-validated by the recorder. Historical loss
reasons remain readable; saved sessions are not rewritten by this change.
