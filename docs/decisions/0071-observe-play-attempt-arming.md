# ADR 0071: Observe play-attempt arming before decision

- Status: Accepted
- Date: 2026-08-29
- Supersedes: ADR 0068 only for keeping a selection handoff implicit before an attempt starts

## Context

The TUI could publish and render a stable music selection before the play-attempt reducer received
the corresponding handoff. A decision transition immediately after the visible stable state could
therefore appear to lose the song, while the observation channel had no pre-decision attempt state
with which to distinguish an unarmed reducer from a later transition failure.

## Decision

Add an `armed` play-attempt state containing the selected song, the stable or held source, and the
causal source sequence. Update the reducer's handoff before publishing the temporal music-select
change, then publish the additive `play_attempt_changed` armed state. Unknown screens preserve the
handoff. A subsequent decision transition consumes that same handoff into the attempt; clearing or
leaving selection clears an armed state.

Raw field observation still precedes temporal presentation, and temporal presentation still
precedes the additive play-attempt event. Recognition authority, accepted events, and persistence
remain unchanged.

## Consequences

- A visible stable selection has already armed the application reducer.
- Operators can distinguish selection arming from decision detection and result linkage.
- Existing event consumers must tolerate the additive `armed` variant as already required by the
  provisional v2 observation contract.

