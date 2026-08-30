# ADR 0080: Rank full-title evidence after active-prefix evidence

- Status: Accepted
- Date: 2026-08-30
- Supersedes: ADR 0046 only for its minimum active-prefix length and prefix-tie ordering

## Context

ADR 0046 rejects an active-list observation whose folded comparison key has fewer than five
Unicode scalar values. Retained target diagnostics show that this rejects valid complete catalog
titles such as `X`, `〆`, and `無双` before their catalog evidence is ranked. The active catalog is
the complete and authoritative candidate set for the selected game environment, so a short
observation is not evidence that the visible song falls outside the lookup domain. An arbitrary
minimum length therefore suppresses valid identity evidence without establishing a distinct trust
boundary.

Prefix evidence alone also cannot distinguish a complete short title from a longer catalog title
with the same prefix. For observation `X`, both catalog titles `X` and `X-DEN` have exact prefix
distance zero and similarity `1/1`, while their already-retained full-title scores distinguish
them. Character-count routing between full-title and prefix policies would introduce another
layout threshold without improving the candidate evidence.

## Decision

The v2 music-select resolver is
`scorepeek-music-select-active-prefix-full-tiebreak-corroborated-v2`. It evaluates every catalog
candidate through the existing prefix and full-title score domains without branching on observed
character count. A comparison-key-empty active title remains typed unknown. The minimum five-unit
gate and `active_list_title_too_short` reason are removed.

Candidates are ordered lexicographically by:

1. prefix edit distance ascending;
2. prefix normalized similarity descending;
3. full-title edit distance ascending;
4. full-title normalized similarity descending; and
5. stable `ScorepeekSongId` order for deterministic representation only.

The active survivor set contains all candidates equal through the first four evidence dimensions;
song-ID order never resolves identity. The selected candidate must still have prefix edit distance
at most one and prefix normalized similarity at least `6/7`. Central-title and artist evidence keep
ADR 0046's strong-evidence thresholds, unique-candidate conflict behavior, and tie-narrowing role.
No weighted score, character-count branch, threshold relaxation, or catalog-external fallback is
introduced.

Consequently, if `X` and `X-DEN` are both in the active catalog, observation `X` selects `X` by
full-title evidence after their prefix tie. If only `X-DEN` exists, it remains the valid exact
prefix candidate. If no catalog title has adequate prefix evidence, resolution remains typed
unknown. Short OCR noise is handled by catalog ambiguity, supplemental conflicts, and the existing
stable-selection temporal reducer rather than an arbitrary length gate.

## Consequences

- Valid one- and two-character catalog titles reach the same evidence ranking and acceptance
  quality checks as longer titles.
- Complete-title evidence only resolves equal prefix evidence; it cannot displace a candidate with
  better prefix distance or similarity, preserving clipped-title behavior.
- Resolver IDs and retained typed reasons identify the semantic change. Existing v1 diagnostics
  remain immutable evidence of the former rejection and can be compared with v2 output.
- Unit coverage fixes the `X`/`X-DEN` coexistence case, the catalog-only `X-DEN` case, no matching
  prefix, comparison-key-empty input, and unresolved equal evidence. Prospective target behavior
  remains unverified until the new binary processes a short-title selection.
