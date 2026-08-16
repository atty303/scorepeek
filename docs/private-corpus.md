# Private corpus contract

This document defines the M2 boundary between immutable private media,
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

Every source binds one opaque capture profile, one immutable domain-normalizer
artifact digest, and one layout profile. These identifiers describe the
observed-to-canonical contract as a whole. Corpus tooling does not infer or
model Wine, Vulkan, Gamescope, compositor, PipeWire, operating-system, or
capture-layer classifications from them.

## Immutable ingest

The input request uses schema `scorepeek-private-corpus-ingest-v1` and contains
only an opaque fixture ID, an opaque session ID, and exactly one profile
binding. For example:

```json
{
  "schema": "scorepeek-private-corpus-ingest-v1",
  "fixture_id": "fixture-001",
  "session_id": "session-001",
  "profile": {
    "capture_profile_id": "gamescope-pipewire-fhd-a",
    "normalizer_artifact_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "layout_profile_id": "infinitas-fhd-layout-a"
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
canonical profile-binding, source, and source-manifest SHA-256 values for
downstream binding.
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
Those operations use the separately approved and version-pinned tool described
below. The stored bytes are the reproducibility boundary even when the original
recording process was manual.

## Pinned media probe and frame extraction

The offline toolchain uses Shaka Project's static FFmpeg binaries at release
`n8.1.2-1`, containing FFmpeg 8.1.2. `mise.lock` pins the platform asset URL and
SHA-256 for FFmpeg and ffprobe; mise also verifies GitHub artifact attestations
during installation. The Linux x86-64 pair is about 92 MiB and fully statically
linked. The build reports GPL version 3, enables GPL/version3 components, and
does not enable nonfree components. It is an offline development/corpus tool,
not a Rust dependency or game-session bundle. This was selected over the roughly
649 MiB conda prefix, a nonfree Aqua build, rolling BtbN snapshots, and source
builds.

Probe a stored fixture into a new private manifest:

```text
mise run corpus:media:probe -- --store /absolute/private/store --output /absolute/private/probe.json fixture-001
```

`scorepeek-private-media-probe-v1` binds the canonical source manifest and
source object to the exact FFmpeg/ffprobe binary digests, video dimensions,
source time base, the sole video stream's explicit index, and every decoded
video frame's contiguous decode index and integer PTS. Media with zero or
multiple video streams is rejected rather than selecting one implicitly. Probe
accepts only a self-contained Matroska container, streams its bytes to ffprobe
through stdin, forces the Matroska demuxer, and allowlists only the `pipe`
protocol. It therefore cannot follow a media-supplied network URL or secondary
filesystem path. Output is bounded to 64 MiB and 250,000 frames. Tool stdout
and stderr are drained with fixed bounds and every process has a ten-minute
timeout; errors expose only status and a stderr digest, not private decoder
text or paths.

Extraction takes a strict `scorepeek-private-frame-extraction-v1` request. It
repeats the fixture, source-manifest, and probe digests and supplies a non-empty
strictly increasing selection of `{frame_id, decode_index, source_pts}`. The
decode-index/PTS pair must match the probe exactly. Before decoding, the tool
reloads the fixture's current canonical source manifest and requires the probe's
source object and complete capture/normalizer/layout profile binding to match
it exactly. Run extraction into a new path:

```text
mise run corpus:media:extract -- --store /absolute/private/store --output /absolute/private/new-extraction /absolute/private/probe.json /absolute/private/extraction-request.json
```

At most 512 selected frames and 4 GiB of RGB payload are admitted. FFmpeg emits
RGB8 P6 PPM without frame-rate resampling. As with probing, the source is sent
through stdin with only the `pipe` protocol enabled and the Matroska demuxer
forced. The tool re-parses every PPM header, checks dimensions and exact pixel
byte count, records pixel-payload and whole-file SHA-256 values, and publishes
a canonical extraction manifest that retains the capture/normalizer/layout
profile binding and selected video-stream index. The new directory uses mode
`0700`, files use `0600`, an existing destination is never accepted, and
files/directories are synced before success. A mode-`0600` parent writer lock
serializes recovery and publication. Recovery removes only staging and
incomplete destinations carrying exact scorepeek ownership markers. Atomic
no-clobber file and directory publication prevents an existing destination
from being replaced. The manifest's `ExtractorIdentity` uses FFmpeg 8.1.2, the
media-probe digest, and the canonical request digest.

## Scorepeek-owned layout measurement

A `scorepeek-private-layout-measurement-v1` request binds an extraction-manifest
digest and must repeat the extraction's layout-profile ID. Each uniquely
ordered field supplies a field type, presence predicate, and human-measured
rectangles bound to distinct extracted frame IDs. The command never derives a
rectangle from pixels and has no upstream coordinate input:

```text
mise run corpus:layout:measure -- --output /absolute/private/layout-measurement.json /absolute/private/new-extraction /absolute/private/layout-request.json
```

Layout measurement accepts only the canonical 1920x1080 frame contract. For
each field it emits the component-wise lower median rectangle, maximum observed
absolute deviation for x/y/width/height, sample count, and opaque sample frame
IDs with their measured rectangles. The output is measurement evidence for a
future versioned `LayoutProfile`; it does not silently activate or replace a
runtime profile. Its new output path must be outside the extraction directory,
so publication and crash recovery cannot mutate the extracted evidence set.

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

## Complete-label authoring

Author one complete-label document through the private store instead of writing
directly into `labels/`:

```text
mise run corpus:label:author -- --store /absolute/private/store /absolute/complete-label.json
```

The command bounded-reads and strictly validates the selected result,
music-select, or non-recognition shape, normalizes it to canonical JSON, and
publishes it as `labels/<sha256>.json` under the existing corpus writer lock.
Publication is idempotent, uses mode `0600`, recovers only scorepeek-owned label
staging entries, enforces the existing 250,000-document/4 GiB label-store
bounds, and fsyncs the object and parent directory before success. Its
`scorepeek-private-complete-label-summary-v1` output contains only the opaque
frame ID, annotation revision, non-personal shape class, canonical byte count,
and label digest; it never returns labelled field values. Intrinsic validation
happens at authoring time. Exact frame, annotation, and screen-class binding is
checked again when a replay suite refers to the digest.

## Replay metadata

Before assembling a replay suite, generate each replay index from strict frame
metadata:

```text
mise run corpus:index:generate -- --store /absolute/private/store /absolute/index-plan.json
```

The `scorepeek-private-corpus-index-plan-v1` input names exactly one stored
fixture and its canonical source-manifest SHA-256. It binds the extractor
identity, time base, and ordered frame metadata already required by the replay
contract. In place of a caller-selected episode ID, each frame carries an
opaque `episode_sha256`. The generator uses that digest as the canonical
episode ID and rejects an episode group that reappears after a different group
has begun. Decode indexes must still increase strictly.

Generation revalidates the stored source bytes and every referenced complete
label before publishing canonical JSON to
`indexes/<replay_index_sha256>.json`. Publication shares the corpus writer
lock, uses private permissions and fsync boundaries, recovers only owned index
staging files, and is idempotent for the same bytes. The index store admits at
most 1,024 objects, 32 MiB per object, and 4 GiB total. Its aggregate-only
summary contains the fixture ID, index digest, and frame and episode counts.
The generated index is directly usable as one entry in a replay suite; suite
assembly remains explicit because split-contract selection is a human dataset
decision.

`scorepeek-private-corpus-replay-suite-v1` is the corpus-wide validation unit.
It contains an explicit `in_profile` or `profile_disjoint` split contract and
one or more `scorepeek-private-corpus-replay-v1` indexes. Each suite binds one
sealed corpus-generation SHA-256, and each index binds the exact canonical
source-manifest SHA-256 to one extractor identity, its version, exact
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
non-increasing per-source decode order and any session ID, episode, session
hash, play hash, title hash, or identical-frame digest assigned across multiple
splits anywhere in the suite. `in_profile` permits the same capture profile ID
in multiple splits so frozen holdout data can measure recognition within an
observed domain. `profile_disjoint` requires each capture profile ID to appear
in only one split even when its normalizer or layout binding differs, measuring
transfer to an unseen observed domain.
Before these checks, the suite's fixture/source-manifest set must exactly equal
its sealed generation. The title-group rule is the enforceable boundary for a
title-disjoint OCR holdout; it does not infer a title from private content.
Replay indexes must use the generation's unique fixture-ID order so the
canonical suite digest is invariant to caller traversal order.

```text
mise run corpus:replay:validate -- --store /absolute/private/store /absolute/replay-suite.json
```

The command outputs a dedicated
`scorepeek-private-corpus-replay-suite-summary-v1` result containing the sealed
generation digest, canonical replay-suite digest, opaque suite ID, index and
frame counts, selected split contract, and per-split counts. It does not emit
paths, media, complete labels, recognized values, or personal data.

## Catalog-independent synthetic title set

Render a deterministic synthetic title-crop set from a seed-only request:

```text
mise run corpus:synthetic:render -- --output /absolute/new/output-directory /absolute/synthetic-request.json
```

`scorepeek-synthetic-title-request-v1` contains only an opaque set ID, a
lowercase SHA-256 seed, and a sample count from 1 through 256. It deliberately
has no text, font, image, or catalog input. The versioned
`scorepeek-procedural-5x7-v1` renderer derives ASCII n-gram labels, glyph style,
gradient background, shadow, and bounded noise from the seed and sample index,
then writes fixed RGB8 512x96 P6 PPM crops plus a canonical manifest. An
existing output path is never overwritten. Files and the output directory are
world-readable (`0644`/`0755`) because this path contains generated data only;
the renderer does not read the private corpus store.

This baseline provides byte-deterministic, independently created renderer and
manifest contracts without adding a font, image, or media dependency. It is
not a claim that the limited procedural glyph domain is representative enough
to train the production OCR model. Expanding glyph coverage or adding an
external font still requires immutable provenance, a redistribution grant,
and the dependency approval described below. The repository's current lack of
a public license remains unchanged; this command does not itself grant rights
to redistribute scorepeek or its generated files.

## Not yet implemented

- real-capture layout-profile adoption and replay execution against recognition
  code;
- production synthetic variation and glyph coverage backed by an approved
  redistributable font or independently authored equivalent;
- Python training, evaluation, ONNX export, and Rust parity gates.

Any media, image, training, or runtime dependency must be proposed with its
pinned version, license, alternatives, and host/bundle impact before addition.
