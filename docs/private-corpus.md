# Private corpus contract

This document defines the first M2 boundary between immutable private media,
offline corpus tooling, the future training/export pipeline, and the scorepeek
game-session core. Real media, extracted frames, complete labels, and replay
indexes remain outside the repository.

## Ownership boundary

- `scorepeek` is the Rust game-session core. It does not depend on corpus or
  training tooling.
- `scorepeek-corpus` is an offline Rust binary for private ingest and replay
  metadata. It does not run during a game session.
- The future Python training/export environment consumes explicitly exported
  private corpus artifacts and produces pinned ONNX artifacts. Python is not a
  runtime fallback.
- Only opaque fixture IDs, opaque group IDs or hashes, non-personal class
  labels, content hashes, schemas, and synthetic contract fixtures may be
  committed. A content hash is a reference, not permission to publish its
  content.

Windows VM recordings and Linux captures have disjoint typed profiles:

- `windows_semantic_reference` covers screen semantics, transitions, closed
  classes, and annotation workflow. It is not capture-calibration or Linux
  release-gate evidence.
- `linux_capture_calibration` binds capture, normalizer, and layout profile IDs
  and is the only role that can support backend/layout calibration after the
  target-machine gates pass.

The two roles cannot silently substitute for each other.

## Immutable ingest

The input request uses schema `scorepeek-private-corpus-ingest-v1` and contains
only an opaque fixture ID, an opaque session ID, and exactly one typed profile.
For example:

```json
{
  "schema": "scorepeek-private-corpus-ingest-v1",
  "fixture_id": "fixture-001",
  "session_id": "session-001",
  "profile": {
    "kind": "windows_semantic_reference",
    "recording_profile_id": "windows-vm-fhd-v1"
  }
}
```

Run ingest with an explicit absolute external store path:

```text
mise run corpus:ingest -- --store /absolute/private/store /absolute/source.media /absolute/request.json
```

Ingest streams the source into `content/<sha256>/source.media`, then writes the
canonical `scorepeek-private-corpus-source-v1` manifest to
`manifests/<fixture_id>.json`. Store directories use mode `0700`; source,
manifest, and lock files use mode `0600`. A per-store writer lock serializes
recovery and publication. Newly created files and relevant directories are
synced before success is reported. The aggregate-only command result includes
both source and canonical source-manifest SHA-256 values for downstream binding.
The root, managed directories, and writer lock must be real filesystem entries;
symlinks are rejected before permissions or private content are changed.

The same bytes and request are idempotent. An existing fixture ID cannot be
rebound to different bytes or metadata. Existing identical content remains
usable at capacity; new content is limited to 64 GiB per source, 1,024 source
objects, and 1 TiB total. Fixture manifests are separately limited to 1,024
files and 64 MiB total so content reuse cannot bypass the binding bound. These
are storage safety bounds, not recommended recording sizes.
All required capacity is checked before publishing a new content object. The
reuse path explicitly removes its complete staging copy and reports cleanup
failure instead of returning a successful binding.

Ingest deliberately does not inspect, decode, transcode, or extract the media.
Those operations require a separately approved, version-pinned media tool. The
stored bytes are the reproducibility boundary even when the original recording
process was manual.

## Immutable corpus generation

After ingesting all sources for one dataset generation, seal the complete
current fixture binding set under the same writer lock:

```text
mise run corpus:generation:seal -- --store /absolute/private/store generation-001
```

`scorepeek-private-corpus-generation-v1` contains an opaque generation ID and
the uniquely ordered set of every fixture ID plus canonical source-manifest
SHA-256 present at sealing time. The generation is stored by its own canonical
SHA-256 with private permissions and fsync publication. Later ingests do not
rewrite it. A replay suite names this digest and must contain exactly one index
for every binding in that immutable generation; an arbitrary subset cannot
receive a corpus-wide validation summary. Existing identical generations remain
usable at the generation-store limit of 128 files, 256 KiB each, and 32 MiB
total; a new generation fails without changing older generations.

## Replay metadata

`scorepeek-private-corpus-replay-suite-v1` is the corpus-wide validation unit.
It contains one or more `scorepeek-private-corpus-replay-v1` indexes. Each
suite binds one sealed corpus-generation SHA-256, and each index binds the exact
canonical source-manifest SHA-256 to one extractor identity, its version, exact
extractor-manifest and parameter hashes, source time base, and a sequence of
selected frames. Every frame records:

- opaque frame and episode IDs;
- source PTS and a strictly increasing decode index;
- frame content SHA-256;
- non-personal screen class;
- `train`, `validation`, or `holdout` assignment;
- private session, play, and title group hashes;
- annotation revision and complete-label document SHA-256.

Complete label values stay in the private store under
`labels/<sha256>.json`; the replay index carries only their immutable digest.
Each mode-`0600` document uses the strict
`scorepeek-private-complete-label-v1` schema and is tagged as `result`,
`music_select`, or `non_recognition`. Result and music-select documents contain
their shape-specific mandatory fields as explicit
`known(value)`, `unknown(reason)`, or `not_applicable` states; mandatory fields
reject `not_applicable`. A non-recognition document explicitly distinguishes a
transition, negative scene, or unknown scene. Every document also binds its
opaque frame ID and annotation revision.

Replay validation bounded-reads the named label document, checks its canonical
SHA-256, schema, frame identity, annotation revision, screen-class/shape match,
shape-specific required fields, typed known values, result play-mode/type
compatibility, and `current_score <= 2 * notes`. Unknown counterpart values do
not cause a relationship to be guessed. The corpus-wide check also verifies
mode, filename digest, canonical schema, and intrinsic constraints for every
unreferenced label object. It never emits private field values. The labels
store is fail-closed at 64 KiB per document, 250,000 documents, and 4 GiB total.

Validation reads each named manifest and content-addressed media object from the
explicit store, verifies their bytes and duplicated index metadata, and rejects
duplicate fixture/frame IDs or non-canonical hashes. It also rejects
non-increasing per-source decode order and any session ID, capture profile,
episode, session hash, play hash, title hash, or identical-frame digest assigned
across multiple splits anywhere in the suite. Before these checks, the suite's
fixture/source-manifest set must exactly equal its sealed generation. The
title-group rule is the enforceable boundary for a title-disjoint OCR holdout;
it does not infer a title from private content. Replay indexes must use the
generation's unique fixture-ID order so the canonical suite digest is invariant
to caller traversal order.

```text
mise run corpus:replay:validate -- --store /absolute/private/store /absolute/replay-suite.json
```

The command outputs a dedicated
`scorepeek-private-corpus-replay-suite-summary-v1` result containing the sealed
generation digest, canonical replay-suite digest, opaque suite ID, index and
frame counts, and per-split counts. It does not emit paths, media, complete
labels, recognized values, or personal data.

## Not yet implemented

- media probing and PTS/decode-order frame extraction;
- deterministic episode/index generation and label authoring workflow;
- independently redistributable synthetic rendering;
- replay execution against recognition code;
- Python training, evaluation, ONNX export, and Rust parity gates.

Any media, image, training, or runtime dependency must be proposed with its
pinned version, license, alternatives, and host/bundle impact before addition.
