# ADR 0050: Cache and fetch the registered small model globally

- Status: Accepted
- Date: 2026-08-26
- Supersedes: ADR 0003 and ADR 0006 only for runtime model auto-download; ADR 0031 only for
  requiring a caller-supplied model location and prohibiting download before its loader; ADR 0049
  only for treating the model as manually transferred operator data

## Context

The selected runtime model is not private operator data. PP-OCRv6-small is an Apache-2.0 official
bundle registered at repository revision `b8f84f0b80c529de40b4fbb3544b84fa7233a513` with exact URLs, sizes and SHA-256 values for
`inference.onnx`, `inference.json` and `inference.yml`. Requiring each command and target machine to
receive a manually transferred model directory makes this reproducible public dependency harder to
use without improving its existing integrity contract. The private catalog has a different source
and redistribution boundary and remains operator-managed data.

Scorepeek is not useful in ordinary operation without OCR, and the runtime selects only this small
model. Per-command bundle and identity arguments therefore expose choices that the runtime does not
actually support.

## Decision

Exact `--help`, `--version` and `doctor` invocations run without model initialization. Every other
CLI invocation synchronously calls one common `ensure_small_model` before command dispatch, without
classifying whether the command will later perform recognition.

The normal bundle location is
`$XDG_CACHE_HOME/scorepeek/models/bundles/<registered-manifest-sha256>`, falling back to
`$HOME/.cache/scorepeek/models/bundles/<registered-manifest-sha256>`. An existing completed
directory is reused without network access or automatic replacement. The existing recognition
loader remains responsible for reading and verifying the registered files when inference starts;
an invalid completed cache is not deleted or overwritten automatically.

When the directory is absent, scorepeek downloads only the registered three files over HTTPS. Each
response is bounded by its registered size and must match that exact size and SHA-256. One cache
writer lock serializes publication; a waiter rechecks the completed path after locking. Verified
files are written to a scorepeek-owned marked staging directory and atomically renamed. Failed
HTTP, timeout, size, digest or publication leaves no completed directory. Recovery removes only
marked scorepeek-owned staging. Download start, completion and failure are written to stderr, never
to command JSON or NDJSON stdout. The shared content-addressed bundle store retains the existing
limits of eight generations, 192 MiB per object and 512 MiB total; existing identical content stays
usable at capacity and new content fails before download.

Normal command-specific `--bundle`, `--model-sha256` and `--runtime-sha256` arguments are removed.
The executable records the registered small-model and runtime digests in diagnostic bindings. A
single leading development override, `scorepeek --model-bundle DIRECTORY <command...>`, disables
download and requires the complete fixed small-bundle contract at that absolute directory. It does
not select another model or provide fallback. Candidate manifests and explicit offline comparison
tools for tiny, medium and v5 remain reproducibility evidence and are not connected to normal
runtime selection.

The Python official-model tooling uses the same XDG cache base by default. The former
`$XDG_DATA_HOME/scorepeek/models` location is neither migrated, removed nor consulted as fallback.
The cargo-dist archive continues to exclude all model bytes. No model-management command,
background download, installer, tag, public release or release workflow is added.

## Consequences

- The first ordinary invocation requires network access; deleting the cache causes the next one to
  fetch again. Offline use requires one successful ordinary invocation while online.
- Catalog acquisition and distribution remain unchanged operator-data workflows.
- `--help`, `--version` and `doctor` remain usable in a clean isolated environment without network.
- The cache is disposable, while immutable source revision, Apache-2.0 attribution, size and digest
  registration remain the reproducibility and integrity contract.
