# Diagnostic store controls

`scorepeek diagnostic status` and `scorepeek diagnostic list` are read-only
application controls over one existing diagnostic-run root. They do not run
recognition, infer an INFINITAS session, apply retention, freeze or delete a
run, or export pixels.

```text
mise run diagnostic:status -- --root /absolute/existing/diagnostic-root
mise run diagnostic:list -- --root /absolute/existing/diagnostic-root
```

`status` reports the fixed local policy, actual managed bytes, remaining bytes
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

Inspection is bounded to 8,192 run directories, 50,000 regular files per run,
64 KiB for `run.json`, and 16 MiB for `manifest.json`. Run IDs and schemas are
strict. Symlinks, nested directories, unmanaged root entries, over-bounds
documents, byte-accounting mismatches, and a root or run directory that changes
during any part of inspection fail the whole command with a value-free error.
Per-run or aggregate policy overflow also fails instead of returning zero
remaining capacity as healthy state. The controls
do not rehash every QOI/fact artifact; strict replay or a future explicit verify
control remains the integrity boundary for artifact contents.

Aggregate retention, active-run ownership, freeze, digest-confirmed delete, and
create-only local export remain subsequent controls. Until those controls are
implemented, `remaining_bytes` is observation only and no run is removed.
