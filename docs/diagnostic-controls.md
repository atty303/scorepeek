# Diagnostic store controls

`scorepeek diagnostic status` and `scorepeek diagnostic list` are read-only
application controls over one existing diagnostic-run root. They do not run
recognition, infer an INFINITAS session, freeze or explicitly delete a run, or
export pixels. Retention itself is application-owned and runs automatically
under the store writer lease before and during a new diagnostic run.

```text
mise run diagnostic:status -- --root /absolute/existing/diagnostic-root
mise run diagnostic:list -- --root /absolute/existing/diagnostic-root
```

`status` schema v2 reports the fixed local policy, whether an exclusive writer
lease is currently held, actual managed bytes, remaining bytes
under the 8-GiB aggregate budget, and aggregate completeness/priority counts.
`list` reports only opaque run IDs, the exact `run.json` SHA-256, an optional
completion-manifest SHA-256, terminal status/completeness, priority, and managed
bytes. It does not expose paths, pixels, OCR text, song/player values, replay
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
and one path-derived, zero-byte ownership anchor in its stable parent for the
entire run; status takes the same locks in shared mode while taking an idle
snapshot. A legacy root without the parent anchor remains read-only under its
root lock, while the first writer durably creates the anchor. The root marker is
an inventory sentinel, not the lease identity. Scorepeek processes derive and
honor the same parent anchor, so cooperative cross-process writers serialize
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

Operator freeze, digest-confirmed explicit delete, and create-only local export
remain subsequent controls. Full artifact rehash also remains an explicit
verify/replay responsibility.
