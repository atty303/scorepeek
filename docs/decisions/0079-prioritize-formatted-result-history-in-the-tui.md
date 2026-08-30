# ADR 0079: Prioritize formatted result history in the TUI

## Status

Accepted.

## Context

ADR 0078 added a typed `result_detected` event, but the ordinary TUI only retained its raw JSON
value in the run snapshot. The renderer continued to devote its largest panel to frame-local OCR
and temporal state. A completed play is the operator's primary result, and replacing one latest
value on every result would also hide earlier plays from the same watcher invocation.

The accepted domain event contains the catalog song ID and complete play result, while the preceding
temporal transition contains the catalog title and artist presentation. Both records are reduced by
the same application state before rendering. No catalog or recognizer dependency is needed in the
renderer.

## Decision

- The run view retains the newest 32 `result_detected` records across Gamescope session boundaries
  within one `scorepeek run` invocation. It evicts the oldest entry at capacity and never loads
  historical run artifacts at startup.
- Each entry joins the domain event to the matching stable catalog presentation. The TUI renders
  newest first with an ordinal, catalog title and artist, clear type, SP/DP chart, difficulty,
  level, notes, EX score, theoretical maximum, and score percentage. If the matching presentation
  is unavailable, the stable song ID is the visible fallback rather than guessed catalog text.
- After the first accepted result, the play-results panel precedes play-attempt and raw-recognition
  panels and receives the flexible vertical area. Before that event, the existing layout remains
  unchanged rather than spending space on an empty panel. Compact terminals still show the newest
  formatted result before lower-priority diagnostic detail.
- The TUI is a human-facing result surface and never renders the JSON event payload. The existing
  observation channel and retained run-event artifact remain the machine-readable diagnostic
  surfaces and keep their failure behavior independent from rendering.

## Consequences

One ordinary watcher invocation provides a bounded, readable play history without adding persistent
score storage or changing the provisional event contract. Restarting `scorepeek run` begins a new
TUI history; retained artifacts remain the source for later diagnostic replay. The history includes
personal play results but stays on the same operator-owned local TUI and observation snapshot as the
individual result events.
