# ADR 0052: Watch Gamescope without owning it

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0040 for its single-session, no-reconnect and stdin-stop contract; ADR 0051 only
  for the ordinary `run` lifecycle

## Context

An ordinary user should be able to leave scorepeek running independently of the order and number
of game launches. A two-second source lookup followed by process exit makes Gamescope startup order
an accidental control protocol. Reading `stop` from stdin also does not fit a foreground process,
user service, or terminal's normal signal lifecycle.

Gamescope, Steam, and INFINITAS belong to the user. Ordinary scorepeek operation therefore must not
start, signal, terminate, or restart them. The guided setup process is different: its sole purpose
is to own one dedicated marker-only calibration Gamescope, and ADR 0051 continues to govern it.

## Decision

`scorepeek run [--profile NAME] [--no-recording]` is one application-owned watcher invocation. It
prepares the selected profile, fixed model and ordinary-run lock once, then performs bounded
observations of the default PipeWire remote until SIGINT or SIGTERM. Zero candidates waits. More
than one exact `node.name=gamescope`, `Video/Source` candidate is ambiguous and waits. Exactly one
candidate may be admitted.

One uninterrupted numeric node lifetime is attempted at most once. Exact admission creates one new
capture generation, diagnostic run, field worker and recognition artifact. Source loss normally
finishes that session and returns the invocation to waiting. A terminal stream, normalization, or
recognition failure finishes only that session; the same node is not retried until disappearance
has been observed. A later source lifetime receives a new generation and session ID. The profile is
fixed for the invocation, while the installed Gamescope version, observed capture contract and
active catalog are checked again before each session. A temporary catalog or PipeWire failure is
retried at a bounded interval without repeated output while the state is unchanged.

Routine stdout uses `scorepeek-run-event-v2` NDJSON. It distinguishes watcher start/stop, session
start/finish and field observations. Every session record carries its session ID and capture
generation. Idle and retry states are excluded. Session failure is not an invocation failure;
ordered signal shutdown exits successfully. Startup profile/resource errors and unusable stdout
remain invocation failures.

`signal-hook` registers SIGINT and SIGTERM against scorepeek's existing atomic stop token. Handler
registration is removed by its owner, and no signal is forwarded. Stdin has no control meaning.
Active shutdown preserves receiver, provider, pending-field, diagnostic and artifact finalization
order. Version 0.4.4 is used without default features; it is MIT/Apache-2.0 licensed and adds no
host library requirement.

When recording is enabled, one atomically replaced
`$XDG_STATE_HOME/scorepeek/watcher-status.json` retains the invocation ID, current low-cardinality
state, session count, active-session link, the last 32 state transitions and a dropped count. It
does not retain argv, pixels, catalog strings, or arbitrary PipeWire properties. `--no-recording`
does not write watcher status, diagnostics, or recognition artifacts. Recognition-store capacity
is checked per session; existing artifacts are not deleted, and capacity degrades only that
session's artifact while recognition and watching continue.

## Consequences

- Scorepeek may be started before Gamescope and left running across sequential game launches.
- Concurrent Gamescope sources remain visibly ambiguous rather than being guessed or captured in
  parallel.
- A failed node lifetime cannot create an immediate reconnect loop.
- Real Bazzite verification of both startup orders, two sequential lifetimes, simultaneous sources,
  and idle/active SIGINT/SIGTERM remains required before lifecycle support is claimed.
