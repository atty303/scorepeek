# Private canonical recording corpus

A corpus input is one completed canonical recording directory. It contains
`canonical-manifest.json`, `canonical-ticks.ndjson`, and the declared lossless
Matroska segments. The manifest uses `scorepeek-canonical-session-recording-v5`.
The reader accepts the runtime's lossless `libx264rgb -crf 0` H.264 stream and
FFV1 synthetic segments at the fixed frame shape.
The canonical frame contract is contiguous RGB8 at 1920x1080. Each tick carries
its input sequence, source sequence and timestamp, screen observation, semantic
episode ID, and either a retained frame or a typed elision. The manifest carries
a session-local ID, segment and tick-index integrity, completion, and one of the
game-version states `identified`, `not_observed`, `ambiguous`, or
`observer_failed`. An identified version remains the recorded external fact.

`scorepeek run --record` writes this directory under the same retained
`$XDG_STATE_HOME/scorepeek/diagnostics/<run-id>/` envelope as the runtime
diagnostic stream. The two files have separate contracts. The recording does
not contain capture profile, normalizer, FFmpeg, model, catalog, runtime binary,
run ID, or prior domain outputs. Diagnostic stream publication failure cannot
change recognition or public event authority. Import selects the completed
canonical directory itself; it never searches a run directory or reads the
runtime diagnostic stream.

## Import and review

Import verifies the manifest, tick chronology, complete segment coverage,
encoded SHA-256 and byte length, video shape and codec, and decoded frame
count before publishing an imported generation. It copies verified bytes into
a store-owned staging directory, verifies the copy, and publishes the session
and a review draft. The source remains read-only, including on failure.
Import does not activate a regression oracle.

```text
mise run corpus:operations -- import --store /absolute/private-corpus --recording /absolute/recording/canonical
```

The removable `migration_legacy` module reads a v4 canonical recording directory
and its matching `scorepeek-private-capture-session-v4` document. It verifies
canonical artifact digests and retained-frame bindings from that document,
converts into a new v5 recording, and imports that recording. Neither v4 input
is modified. No live crate reads v4 for this path.

```text
mise run corpus:operations -- import --store /absolute/private-corpus --recording /absolute/v4-recording/canonical --v4-session /absolute/v4-session.json
```

For a v4 session already imported into an old corpus store, supply its session
digest and the old store root instead. The migration reader verifies
`sessions/<digest>.json` and reconstructs the recording from `objects/<sha256>`.
If the old store kept segment objects remotely, first place those exact object
bytes in a separate read-only directory named by SHA-256 and pass it as
`--v4-objects`. Missing or changed objects fail before the new corpus
generation is published. This compatibility reader is confined to the
removable corpus `migration_legacy` module; it does not modify the old store or
make runtime support v4.

```text
mise run corpus:operations -- import --store /absolute/new-private-corpus --v4-store /absolute/old-private-corpus --v4-session-sha256 SESSION_SHA256 --v4-objects /absolute/v4-object-mirror
```

Some v4 capture sessions bind an older v2 canonical recording under
`recognition/` instead of the v4 canonical artifact kinds. The same removable
module verifies the old session and its v2 manifest, tick index, and segment
bindings, then converts the canonical input to v5. The v4 session supplies the
recorded game-version state. The old store and optional object mirror remain
read-only; the capture profile and runtime diagnostic trace are not inputs.

```text
mise run corpus:operations -- import --store /absolute/new-private-corpus --v2-store /absolute/old-private-corpus --v2-session-sha256 SESSION_SHA256 --v2-objects /absolute/v2-object-mirror
```

The import command prints the session digest and review draft path. The
operator edits a separate regression label document and applies it in a second
operation. `review apply` checks the stored draft and session binding; only
`include` publishes the session to `active.json`. Applied labels are immutable.
The current label schema is `scorepeek-private-canonical-regression-label-v1`
with `session_sha256`, `disposition` (`include` or `exclude`), and ordered
`transitions` entries containing input `sequence` and `screen`, plus
`domain_event_count` and `domain_event_sha256` from the ordered core event output.
The digest binds each serialized event to its coordinator input sequence without
keeping session-wide event copies in memory. OCR queue and inference durations
are excluded from this semantic digest; recognized values and domain outputs
remain included.
The same explicit read-only replay target can print a report for one recording
when authoring a reviewed label:

```text
cargo test --locked -p scorepeek-corpus --test full_replay -- --recording /absolute/private-corpus/sessions/SESSION_SHA256
```

```text
mise run corpus:operations -- review apply --store /absolute/private-corpus --draft /absolute/private-corpus/sessions/SESSION_SHA256/SESSION_SHA256.review.json --labels /absolute/reviewed-label.json
```

No real recording, player data, model bytes, catalog generation, or complete
private label belongs in Git. This workflow does not provision a private corpus
for CI. Do not start a new `scorepeek run` while importing from a run that may
be rotated; a disappearing or changed source fails import without publishing a
new active generation.

## Replay

The repository-created synthetic recording test exercises canonical reading,
segment decoding, the recorded game-version state, current screen predicates,
semantic episode chronology, ordered core coordinator inputs, bounded output, oracle
comparison, and failure. It runs under ordinary `cargo test --locked --workspace`
and needs no private data. A full active generation runs only through the named
custom Cargo test. Its exit status is nonzero for invalid input or oracle
mismatch.
Result and Music Select frames use registered OCR, the embedded numeric model,
and the current catalog registered by this source tree. Replay acquires the
registered catalog and model files into an isolated temporary store on first
field observation. It does not select resources from the recording, an
arbitrary local path, or active XDG state. A resource or field-observation
failure is a replay failure. The private full replay is an explicit operator
gate.
Replay inspects at most eight frames in parallel per session and runs at most
four active sessions concurrently. Core owns the bounded field and PP-OCR pools;
corpus supplies resolved registered resources and consumes their results. Each
session still submits canonical inputs,
field observations, and core outputs in input order; reports retain suite order.
The explicit replay test prints progress to stderr while verifying segments and
about every 15 seconds during frame processing. Each line shows the recording,
phase, processed and total inputs, retained frames, segments, and elapsed time.

```text
mise run corpus:test -- --store /absolute/private-corpus
```

The custom target has `test = false` and `harness = false`. It has no feature
gate. The single mutating operations binary is available only with the empty
package-local `operations` feature. Routine `mise run test` does not execute
full private replay or corpus operations.
