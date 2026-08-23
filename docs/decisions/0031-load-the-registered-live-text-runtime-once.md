# ADR 0031: Load the registered live text runtime once

- Status: Accepted
- Date: 2026-08-23
- Complements: ADR 0022's PP-OCRv6-small selection and ADR 0030's field-observer worker

## Context

The registered dynamic ONNX decoder verifies a model bundle and constructs an ONNX Runtime session
for each offline request. Reusing that entry point in a live observer would put repeated model,
dictionary, and filesystem work on the observation path. ADR 0030 provides a synchronous loader
before worker startup, but did not define the production resource set or runtime identity that it
must load.

The immutable recognition binding already carries catalog, model, and runtime SHA-256 values. Those
values must select resources actually loaded by the application rather than merely describe them.
The selected model remains an observation source only; loading it must not create a field, song, or
event acceptance policy.

## Decision

The v1 live text resource set consists of:

- the active catalog snapshot whose content digest exactly equals the run's `catalog_sha256`;
- the complete registered PP-OCRv6-small bundle with ONNX model digest
  `5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634`;
- the canonical runtime manifest
  `models/manifests/pp-ocrv6-small-live-runtime-v1.json`, whose artifact digest is
  `4864f57937b6d57510e82234325f611df31521ff508767011de137bebdf531dc`.

The runtime manifest fixes `ort` 2.0.0-rc.13 with API 27, the CPU execution provider with its arena
disabled, one intra-op thread, one inter-op thread, sequential execution, all graph optimizations,
the ADR 0022 dynamic preprocessor, greedy CTC collapse, and the registered model-bundle manifest.
This is the initial exact runtime contract, not a performance claim. Any GPU provider, thread-count
change, allocator change, or other runtime configuration requires a new manifest and digest.

`RegisteredRecognitionResources::load` rejects non-absolute or non-directory locations, model or
runtime binding mismatch, absent or changed active catalog, incomplete or changed bundle files,
and ONNX Runtime initialization failure. It reads no alternate location, downloads nothing, and has
no fallback. It retains the verified catalog, dictionary, and constructed session for the immutable
run. `FieldObserverSessionBinding::load_registered_resources` is the application loader entry point;
it executes synchronously before the worker thread starts.

The reusable runtime can return an open-text observation for one already-owned bounded RGB8 crop.
That value contains only input width, output timesteps, and decoded text. It is not yet connected to
a screen-level field observer and carries no field validity, confidence threshold, catalog match,
song identity, suppression, or accepted-event authority.

The read-only `recognition field-resource-load-gate` exercises the same loader against explicitly
selected locations and digests, transfers the loaded resources into the ADR 0030 production worker,
and requires its bounded teardown to complete without submitting crops. Its bounded JSON result
reports only success or a stable load/worker error type plus the three selected digests; catalog
content, bundle bytes, paths, environment strings, pixels, and arbitrary properties are not JSON
fields. Ordinary typed error causes remain available on stderr and operational identifiers are not
treated as credentials. The diagnostic run remains the application correlation and persistence
owner; this library loader does not create a recorder or remote export.

## Consequences

- Model and catalog filesystem work can be completed once before capture and inference begins.
- The CPU/thread choice is intentionally conservative and must be measured on the target host before
  support or performance claims.
- The offline batch command now shares the same per-crop preprocessing, tensor validation, and CTC
  collapse helper, while retaining its request-scoped session lifecycle.
- A later checkpoint must map complete screen-local crop sets to typed imperfect observations and
  record the loader outcome in the application-owned diagnostic run. It must not reinterpret this
  loader gate as live recognition evidence.
