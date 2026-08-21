# ADR 0024: Limit temporal state to selection song context

- Status: Accepted
- Date: 2026-08-21
- Supersedes: ADR 0023 for `play_attempt` and full-session state inference;
  ADR 0022 only where it names play-attempt transitions as the integration gate

## Context

Music selection and result expose complementary evidence for the same song.
Temporal context is useful because intersecting their bounded candidate sets
can turn two ambiguous screen-local observations into one song-unique result.
It does not follow that scorepeek must reconstruct attempts, mode progression,
retry counts, or the whole INFINITAS session.

INFINITAS standard play repeats selection, gameplay, and result without a
fixed play-count limit. Gameplay can restart without result, and result can
return directly to gameplay for the same song. Dan courses repeat gameplay and
ordinary result a finite but non-fixed number of times and may finish with a
separate dan result. These flows are valuable replay scenarios, but modeling
them as runtime-owned attempts adds ambiguity without improving song identity.

Frame-level recognition also cannot classify every transition or animation.
Treating every unrecognized frame as a hard break would discard selection
context during ordinary play.

## Decision

The stateful recognition boundary owns only the last stable music-selection
candidate set needed to contextualize result song resolution.

- A stable selection installs or replaces the context.
- Confirmed non-state scenes, frames with no semantic anchor, gameplay,
  ordinary result, and either retry shape preserve it.
- Result resolution intersects its screen-local candidate set with the stable
  selection set. One member is accepted with contextual evidence; an empty
  intersection is a typed conflict; multiple members remain ambiguous.
- A screen-local unique result remains acceptable without selection context.
- Selection context cannot establish result-screen presence, savability,
  score, or any other result-only field.
- Result processing does not consume context because result-to-gameplay replay
  can occur without another selection.
- A confidently observed title, session end, recording coverage gap, or change
  to capture/normalizer/layout/catalog/model/runtime binding clears context.
- A failed frame classification is not itself a coverage gap and does not
  clear context.

The recognition core does not own mode, course progress, play count, attempt
identity, retry detection, partial-history composition, or retrospective mode
correction. It emits observed recognition facts; persistence and consumers own
later composition. No `play_attempt` is part of the public event contract.

The operator-supplied launch, title, mode-selection, standard, dan, retry,
return-to-title, normal-exit, and abrupt-exit flows remain explicit validation
scenarios. They test that irrelevant scenes preserve context and real reset
boundaries clear it; they are not discarded merely because they are absent
from the runtime state type. A retained private full-session recording is the
preferred source for refining those scenarios. Recording-derived composition
must be reported for operator review, and recording-external facts remain
separate annotations.

Bounded replayable diagnostics remain an application concern independent of
recognition success. They may measure missed-result denominators, but that
diagnostic purpose does not expand the song-context observer into a session
state machine.

## Consequences

The observer is a small synchronous deterministic reducer with typed inputs and
outputs, so the same recorded inputs can be replayed without a separate
telemetry surface. Live recording, retention, completeness, and export remain
subject to the program observability contract when implemented.

ADR 0023 remains historical evidence for why music-select and result context
and recognition-independent recording matter. Its explicit attempt linkage,
fixed synthetic cadence, and full timeline proposal are no longer current
requirements.
