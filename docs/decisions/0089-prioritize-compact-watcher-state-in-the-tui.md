# ADR 0089: Prioritize compact watcher state in the TUI

- Status: Accepted
- Date: 2026-08-31
- Supersedes: ADR 0084 for panel order and compact accepted-event layout, and ADR 0088 for promotion-panel detail
- Complements: ADR 0058 and ADR 0083

## Context

ADR 0088 limited the TUI to the latest accepted event, but the compact event path still assigned the
whole terminal to that one event. The expanded path reserved fourteen rows even though one v2 event
normally rendered in eight content rows. Watcher state was below the event and retained invocation
and profile identity instead of emphasizing the current screen and lifecycle state.

The promotion panel named the first failed numeric calibration boundary but did not show the typed
score and judgment tuple already present in the same field observation. An operator could see that
promotion was blocked without seeing which other required values were known or unknown.

## Decision

- Put a two-row watcher panel at the top of every TUI layout. Show current watcher state, screen,
  session count, capture generation, and the current message. Show recording status in that compact
  surface when it is not ready. Invocation, profile, and active-session identity remain available in
  the machine-readable snapshot instead of occupying routine TUI rows.
- Size the watcher, latest accepted-event, and promotion panels from their rendered content. A
  compact layout with one accepted event no longer assigns the rest of the terminal to that event.
  Compact layouts with an accepted event omit the lower recognition panel but retain the watcher and
  promotion state when an attempt exists.
- In a result-screen promotion panel, render the typed mandatory numeric tuple as EX, PG, GR, GD,
  BD, and PR. Render a known parsed value directly and an unknown value as `?`. List every enabled
  calibration rejection for those fields, including its boundary kind, observed value, and
  configured threshold.
- Do not render rejected OCR candidate strings. Do not change recognition, temporal reduction,
  attempt linkage, accepted-event publication, observation recording, snapshot shape, or public
  event authority.

## Consequences

The watcher remains visible as the first and smallest operational surface. One accepted event uses
only the rows required by its values, while promotion failures expose the complete mandatory tuple
and all available calibrated rejection boundaries without turning provisional OCR into event
authority.
