# ADR 0047: Operate target machines from private bundles

- Status: Accepted
- Date: 2026-08-26
- Complements: ADR 0029's exact capture-profile admission, ADR 0040's ordinary foreground
  session, and ADR 0043's bounded failure evidence
- Supersedes: the roadmap prohibition on profile authoring automation, only for an explicit
  operator-requested calibration against a scorepeek-owned known marker; ADR 0043 only for
  promoting an explicitly operator-reported recent in-memory observation to durable evidence

## Context

Scorepeek can capture, normalize, observe, and retain recognition evidence on the development
machine, but that is not yet an operator-usable application boundary. The current foreground
command exposes internal digests and provenance fields as mandatory arguments, the checked-in
source tree is needed to assemble its resources, and the existing development-machine bindings
cannot admit a different machine's 4K Gamescope output. Repeating bounded gates on the development
machine does not establish that a new operator machine can be onboarded or used during ordinary
play.

The next development environment is another operator-owned Bazzite machine whose ordinary
Gamescope output is 4K. The first goal is not a public redistributable release. It is a durable
private deployment and improvement loop within the same operator control domain, designed as the
first-user path: install one bundle, explicitly calibrate one profile, run scorepeek during normal
play, transfer only a selected diagnostic run, replay it on the development machine, and return an
updated bundle without requesting another play solely to tune recognition.

## Decision

### Deployment unit

The first target-machine deployment unit is a create-only
`scorepeek-operator-bundle-v1`. Its canonical manifest binds the exact Linux x86-64 executable,
build identity, canonical layout, active catalog snapshot, registered OCR bundle and runtime
manifest, their content digests and sizes, and explicit host runtime prerequisites. It contains no
capture binding, captured frame, player/rival data, credential, raw external source response, or
mutable target-machine state. Catalog strings and resource paths are not suppressed merely because
the bundle is private. Source permission and model license still govern redistribution; the v1
bundle is transferable only inside the operator's personal control domain.

Bundle verification is filesystem-bounded and occurs before capture. A target machine does not
need the repository checkout, mise, Rust, or Python at runtime. Host-provided dynamic libraries,
PipeWire, Gamescope, GPU support, and their accepted versions remain explicit preflight results;
they are not silently vendored or downloaded. Bundle activation is side-by-side and keeps the
previous verified bundle available for rollback. It does not install a service, edit Gamescope or
INFINITAS configuration, or mutate persistent host configuration without a separate operator
action.

### Explicit target-profile onboarding

An operator-requested Gamescope setup operation owns one calibration Diagnostic Run. It preflights
the bundle and host, presents or launches a scorepeek-owned 1920x1080 known marker through an exact
declared Gamescope Wayland configuration, samples the selected PipeWire source, derives the marker
geometry, and publishes one create-only machine-local binding only after an independent marker
comparison passes. The binding retains the exact Gamescope version, backend, output and nested
sizes, refresh, scaler, filter, BGRx format, memory type, stride, and rational normalizer geometry.

This is explicit guided calibration, not runtime auto-calibration. Ordinary sessions never measure
borders, author or switch profiles, relax a contract, or fall back to another route. A 4K output is
a separate profile from the existing 1920x1080 and 2556x1428 development profiles. FSR, NIS,
Reshade, HDR, a different Gamescope version, or different negotiated caps require another explicit
profile and independent evidence.

### Ordinary play

The target-facing operation is `scorepeek run --profile NAME`, with omission of `--profile`
permitted only when exactly one enabled local profile exists. The selected profile owns all
otherwise-internal build, layout, catalog, model, runtime, environment, Gamescope, and normalizer
identities. Preflight or admission failure stops scorepeek before recognition with a stable error
type and an actionable recovery, without changing Gamescope or INFINITAS.

Scorepeek attaches to an operator-started Gamescope session. It never starts, signals, closes, or
restarts INFINITAS or the ordinary Gamescope process. Stopping scorepeek stops only scorepeek and
performs receiver, provider, field-worker, diagnostic, and recognition-artifact teardown in the
existing order. The initial target loop may expose provisional exact recognition observations; it
does not wait for stable event authority before collecting useful evidence, and it does not call
those observations supported events.

### Observation and improvement loop

One scorepeek invocation first owns a bounded start-attempt observation envelope. It distinguishes
bundle preflight, source wait/acquisition, and binding admission, and completes with its own stable
status and error type if admission never creates a capture generation. Exact admission links that
attempt to one ADR 0025 Diagnostic Run; only then is the binding-owned `run.json` published. That
run distinguishes frame reception, normalization, screen inspection, field inference, song
resolution, evidence persistence, and ordered shutdown. A failed pre-admission attempt is never
fabricated as a binding-owned Diagnostic Run. Public result output remains separate from both
observation surfaces. Each failure has one stable typed owner; recording degradation does not
replace capture or recognition results.

Bounded local diagnostics are enabled by default and remain operator-controlled. ADR 0043's
failure-window policy remains the ordinary durable pixel-retention contract: canonical QOI is
sparse, raw BGRx is paired only for selected partial-result or known-screen transitions, and normal
frames are not recorded continuously. In addition, before screen inspection or recognition, the
foreground application keeps a fixed-age, fixed-count, and fixed-byte in-memory problem-report tail
of recent canonical owners plus a smaller bounded set of same-sequence raw BGRx owners. Completed
screen and field observations are linked to matching resident sequences when they exist, but they
do not trigger or gate pixel-tail membership. The exact limits are part of the versioned retention
policy and are preflighted against the target profile's observed frame size.

An explicit operator problem marker made while that tail is resident selects a bounded sequence or
monotonic interval. It atomically claims the matching canonical and available raw owners into a
fixed-count pending problem-report ledger, then returns without blocking capture. A pending report
continues accepting same-sequence screen, field, and recognition links until each selected
sequence reaches worker completion, an explicit queue/drop outcome, worker terminal state, or a
bounded report-finalization timeout. Tail eviction cannot release claimed owners. A report is
manifest-finalized through the existing non-blocking writer only after that watermark is known.

Missing downstream evidence is typed as `screen_observation_unavailable` or
`field_observation_unavailable` only after the watermark proves that no corresponding result can
still arrive. Queue drop, worker failure, and timeout retain their distinct existing typed owners
instead of being collapsed into absence. The report records `source_evidence_unavailable` when the
selected sequence has no claimed raw owner; in that case canonical recognition can be replayed,
but transform correctness for that exact frame remains unverified. Marking does not block capture
or change recognition. A later `freeze` changes retention priority only for bytes already
published and never claims to recover an unrecorded observation. Ledger capacity, queue, flush,
tail eviction, or storage failure marks evidence `partial` or `dropped` without interfering with
play or recognition.

Transfer is explicit and remote export remains disabled by default. A selected diagnostic export
contains the bundle/profile identities, run and artifact manifests, exact OCR/catalog/song/decision
evidence, and only the QOI/raw pairs already selected by ordinary retention or an operator problem
marker. It does not include unrelated runs. On the development machine, transform inspection
reruns the registered normalizer from a same-sequence paired raw frame before recognition replay
uses the same post-canonical production code as live capture. When that raw pair is unavailable,
the export makes the transform boundary explicitly unverified instead of treating canonical QOI as
transform evidence. A fix must replay the reported run and existing frozen suites before a
replacement bundle is activated. Prospective confirmation then occurs during the next natural
play session; another play is not requested merely to tune thresholds, geometry, or recognition
code.

## Delivery checkpoints

1. **Replay completeness**: implement the manifest-bound raw-to-canonical transform inspector and
   prove that retained failure evidence is sufficient without broadening routine retention.
2. **Portable private bundle**: build and verify a current release bundle on a clean compatible
   target environment; report every runtime prerequisite and remove the repository checkout from
   the game-session path.
3. **Guided 4K profile**: make the known marker part of the operator setup path, author one exact
   target-machine 4K Wayland binding, and independently reproduce its canonical marker result.
4. **Routine entrypoint**: reduce ordinary startup to the selected local profile, keep internal
   gates as development verification surfaces, add the pre-recognition bounded recent
   problem-report tail, fixed-count pending-report ledger, and explicit marker, and verify that
   scorepeek teardown cannot control Gamescope or INFINITAS.
5. **Round-trip diagnosis**: mark and export one seeded user-visible failure while its recent
   evidence is resident, then complete development-machine transform and recognition replay,
   replacement bundle creation, target verification, and rollback without a repository checkout or
   a recognition-tuning replay request.
6. **Natural-play qualification**: use ordinary play to measure complete/partial runs, queue and
   retention health, frame age, inference cost, recognition outcomes, and shutdown. Only the
   existing semantic, lifecycle, and performance gates can promote the exact 4K profile from
   diagnostic use to supported use.

## Consequences

- The second machine becomes the first consumer acceptance environment rather than another
  development-only gate host.
- Private same-operator deployment can proceed before public catalog/model redistribution is
  solved; it is not evidence that unrelated users can obtain those resources legally or
  reproducibly.
- Strict binding remains the correctness boundary, while explicit marker calibration removes the
  need for a developer to hand-author rational geometry and digest arguments.
- Event daemon, public release, automatic remote telemetry, host configuration management, and
  arbitrary 4K/HDR/scaler support remain outside this checkpoint.
