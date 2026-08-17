# ADR 0011: Index FFV1 recordings by packet order

- Status: Accepted
- Date: 2026-08-17
- Supersedes: ADR 0010's decoded-frame probing method

## Context

ADR 0010 correctly makes the immutable recording bytes the reusable dataset
root, but the first importer also decoded every video frame to enumerate PTS.
On a 14,785,693,017-byte FFV1 recording, that one derived observation took
about 696 seconds after separating the two SHA-256 passes. The same recording's
video packet index was available in 2.58 seconds and had the same 27,499 count
and boundary PTS values, from 0 through 458300.

Full-source SHA-256 remains necessary for byte identity. Full video decoding is
not necessary to establish that identity, the observed stream contract, or an
FFV1 packet-order index. Recording collection must remain cheap enough that an
operator can preserve complete sessions instead of trimming them to reduce
import time.

## Decision

The destructive `scorepeek-private-media-probe-v4` contract accepts only a
self-contained Matroska recording with exactly one FFV1 video stream. It records
`index_basis: ffv1_packet_order` and assigns contiguous decode indexes from the
demuxed video packet order. Every packet must have an integer source PTS. Zero
packets, multiple video streams, another codec, malformed media, or bounded
output excess fails closed; there is no automatic fallback to a full decoded
probe or another codec-specific assumption.

The importer continues to SHA-256 the external recording, copy it into private
staging while independently recomputing the same SHA-256, and probe only that
verified staging snapshot. The packet-order probe remains bound to the source,
source manifest, capture profile, exact FFmpeg/ffprobe binary digests, stream
index, observed media contract, and recording manifest.

Frame extraction keeps the requested `{decode_index, source_pts}` contract. The
pinned FFmpeg extraction command reports the PTS of every actually decoded
selected frame. Publication succeeds only when the reported count, output
order, and PTS exactly match the packet-order probe. This localizes the reduced
packet-to-frame assumption to FFV1 and checks it for every frame that becomes
derived pixel evidence.

Development-profile SHA-256 dependencies are optimized without changing the
SHA-256 identity scheme. Dataset sealing hashes each object once while building
the generation and does not immediately repeat the same full-source hash in the
same operation. Explicit local verify, push, pull, and remote verify continue to
hash complete bytes.

## Consequences

- Initial FFV1 recording import no longer decodes every frame merely to build a
  PTS index.
- A non-FFV1 recording must be transcoded by the capture workflow before import
  or supported later by a new explicit indexing contract.
- The v3 media-probe schema is removed rather than accepted through a
  compatibility path; recordings are reimported from the preserved source.
- A malformed or unsupported packet-to-frame relationship can survive import,
  but it is rejected before any selected decoded frame is published.
- SHA-256 content identity and complete explicit verification remain available;
  routine operations no longer pay debug hashing or duplicate seal costs.
