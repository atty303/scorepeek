# ADR 0084: Make accepted domain events the primary TUI surface

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0079 only for hiding the result panel before the first event and for its v1 result detail
- Complements: ADR 0058 and ADR 0083

## Context

ADR 0079 made accepted results the highest-priority TUI content only after the first result event.
Until then, watcher state, provisional play-attempt state, frame-local recognition, and observation
channel health occupied the whole terminal without identifying their diagnostic authority. ADR
0083 has since made an accepted play attempt and checked performance breakdown part of the v2
result event. Leaving those values out of the TUI would make the raw recognition path appear more
complete than the accepted result.

## Decision

- Keep an `Accepted play events` panel present for the whole watcher invocation and place it before
  every other panel. Populate it only from `scorepeek-result-detected-v2` records reduced by the
  application. A stable recognition result or confirmed attempt alone never creates a row.
- Render current EX score, clear type, chart, five judgments, typed miss/fast/slow/combo-break
  values, and typed previous-best values from the domain-event payload. Render `not_displayed` as
  `--`, `not_played` as `NO PLAY`, and typed unknown as `?`; diagnostic reasons remain in bounded
  artifacts and observation records.
- Continue to enrich a matching event song ID with the already-reduced catalog title and artist,
  falling back to the event song ID when that presentation is unavailable. This presentation does
  not change event acceptance or payload values.
- Label watcher state, play-attempt state, recognition values, temporal state, resolver evidence,
  and observation-channel health as `Debug`. These panels remain useful for live diagnosis but are
  not score or event authority. Once an accepted event exists, compact layouts dedicate the
  terminal to accepted results and omit all lower-priority debug panels. Compact result labels are
  shortened and never wrapped; narrower layouts split previous-best across two lines.
- Do not change the observation socket, run-event artifact, public `/v1.sock` boundary, recording
  policy, or event suppression behavior.

## Consequences

The TUI has one visually explicit result authority: accepted domain events. Before the first event,
it says that no accepted event exists instead of presenting provisional recognition as a result.
Optional typed states remain distinguishable without exposing OCR text or suppression reasons in
the result panel. This change does not qualify target cadence, installed-binary behavior, or public
event authority.
