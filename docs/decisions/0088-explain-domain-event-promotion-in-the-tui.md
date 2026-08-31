# ADR 0088: Explain domain-event promotion in the TUI

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0079 only for rendering multiple retained results, and ADR 0084 for play-attempt and observation-channel panel presentation
- Complements: ADR 0058, ADR 0083, and ADR 0087

## Context

The accepted-event panel distinguishes domain events from provisional recognition, but its retained
history can displace the evidence needed to understand why the current result did not become an
event. The play-attempt panel describes selection-to-result linkage without identifying the next
unsatisfied promotion gate. Numeric recognition can therefore reject a mandatory field at its
calibrated probability or runner-up-margin boundary while the TUI shows only a confirmed attempt and
no accepted event. A separate observation-channel panel consumes space without explaining that
failure.

## Decision

- Keep the invocation-local bounded result history in `RunViewState`, the observation socket, and the
  run-event artifact, but render only its newest accepted `scorepeek-result-detected-v2` event in the
  TUI. Include the invocation result count so the single displayed event is not mistaken for the
  complete retained history.
- Refine the play-attempt panel into a provisional domain-event promotion tracker. It reports the next
  unsatisfied gate across stable selection, song decision, gameplay, result linkage and stability,
  mandatory numeric acceptance, two-observation numeric stability, and domain-event publication.
- When an enabled numeric calibration rejects a mandatory field, identify the field, boundary kind,
  observed confidence, and configured threshold. Do not render OCR candidate strings in this panel.
  If no field-level calibration rejection is available, retain typed chart, performance, temporal,
  linkage, and suppression reasons as the fail-closed explanation.
- Hide the observation-channel panel from the TUI. Channel health remains available in the
  machine-readable observation snapshot and non-TTY status output, while event evidence remains in
  bounded artifacts; this change does not remove or weaken the channel.
- Do not change recognition, attempt, temporal, event, public `/v1.sock`, recording, or suppression
  authority. The promotion tracker is a reducer-derived explanation of existing typed evidence.

## Consequences

The terminal devotes accepted-result space to the latest event and uses the reclaimed area to show
why the current attempt is still provisional. A confirmed attempt blocked by numeric calibration can
now report, for example, `PGREAT confidence margin 0.56 < 1.31`, while the accepted-event panel remains
empty and authoritative. Complete retained observations continue to support replay and deeper
diagnosis outside the TUI.
