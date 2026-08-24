# ADR 0040: Run a foreground live recognition session

- Status: Accepted
- Date: 2026-08-24
- Complements: ADR 0035's bounded execution gate, ADR 0037's value-bearing local evidence,
  ADR 0039's off-capture-loop artifact writer, and ADR 0029's calibrated lease

## Context

The bounded Gamescope gates prove individual capture and recognition contracts, but they are not
the ordinary live application. In particular, the result-recognition gate stops after at most 60
seconds, requires a result observation, and turns incomplete recognition evidence into gate
failure. Running that gate once against INFINITAS would not prove that the intended live session
stays connected, continuously recognizes frames, exposes values while running, or shuts down its
resources in order.

The first ordinary runtime still precedes accepted event authority and the Unix socket API. It
therefore needs an explicit foreground result surface without inventing accepted events or hiding
the exact OCR and resolver values needed to review the live path.

## Decision

`scorepeek run gamescope` owns one foreground recognition session. It loads the registered catalog,
model, runtime, diagnostic run, and recognition artifact before acquiring one explicitly bound
Gamescope provider. It keeps that calibrated provider and receiver connected until the exact
`stop` control line is read from stdin or a typed terminal capture/recognition failure. It does not impose the validation gates' 60-second
duration and does not reconnect or change capture profiles automatically.

The command uses the same post-canonical `FieldObservationSession`, registered screen-field
observer, full-catalog candidate domain, result resolver, diagnostic bridge, and recognition
artifact serializer as recording simulation and the bounded live gate. Capture remains
non-blocking: inference and artifact I/O stay behind their existing capacity-two workers.

Stdout is an NDJSON result surface. It emits:

- one session-started record after exact binding admission;
- one bounded observation record for each completed field output, containing its live sequence and
  monotonic interval, screen-local exact OCR strings, explicit unimplemented field states, and the
  result-song decision or typed unknown including song ID and reason when present; and
- one terminal session summary after ordered shutdown.

Full catalog strings and per-song candidate metrics remain in the create-only local recognition
artifact rather than being duplicated in every stdout record. Pixels remain in the diagnostic image
store. These separations are about sink purpose and bounded output size, not secret redaction.

An input-control thread accepts only the bounded exact line `stop`; it does not signal Gamescope,
INFINITAS, or the process group. EOF and other input do not request shutdown. The capture loop checks a shared stop token with bounded polling. Requested stop is
distinct from capture, recognition, worker, or storage failure. Receiver shutdown precedes provider
shutdown, pending field results are given the existing bounded finish interval, the field worker is
finished before the diagnostic run, and the recognition artifact is finalized last.

Diagnostic or recognition-artifact degradation never changes or suppresses an already computed
recognition observation. The terminal summary reports capture status, stop reason, field-worker
status, diagnostic completeness, artifact completeness and digest, queue loss, and observation
counts independently. Unlike the value-evidence gate, an ordinary session is not an error merely
because no result screen occurred or evidence became partial. A terminal capture, normalization,
recognition, or field-worker failure remains a process error.

When recording is enabled, the command preflights the diagnostic root before capture. It creates an
absent absolute root with private permissions and emits a typed `diagnostic_status` NDJSON record;
an invalid or unavailable root is reported as degraded without replacing recognition. Each
screen-predicate diagnostic fact retains the bounded numeric predicate values and thresholds,
including unknown screens, so a routing failure can be distinguished from absent OCR output.

## Consequences

- A real INFINITAS run can exercise the intended continuous application path instead of a
  validation-only timeout wrapper.
- The foreground NDJSON stream is provisional recognition output, not the accepted versioned event
  API and not event-authority evidence.
- One provider lifetime is preserved, avoiding an admission probe that would consume the session
  before the real run on providers that do not deliver a first frame to a second receiver.
- Artifact capacity or storage failure is visible and non-interfering; rolling long-term evidence,
  automatic session restart, event deduplication, and the Unix socket remain later decisions.
- Passing one live session does not establish release accuracy, target-host performance, or capture
  profile support.
