# ADR 0046: Resolve music selection from the active title prefix

- Status: Accepted
- Date: 2026-08-25
- Complements: ADR 0034's full-catalog metrics, ADR 0037's value-bearing local evidence, and ADR
  0038's independent result-song resolver

## Context

The music-select screen presents the selected song through three different visual domains. The
active list row is ordinary one-line text, but the UI clips long titles at its right edge. The
large central title is a song-specific texture whose line count, placement, and decoration are not
uniform. The artist is ordinary text but can be short, stylized, or imperfectly decoded. Treating
the three OCR strings as interchangeable votes or adding weighted edit scores would therefore
hide materially different evidence quality behind tunable weights.

Retained direct-Wayland evidence contains both failure shapes. `MOVE! (We Keep It Movin')` was
decoded exactly from the active row while the previous central-title crop omitted its left edge and
the artist was garbled. `ASIAN VIRTUAL REALITIES (MELTING TOGETH` was decoded from the active row as
the exact visible prefix of the longer catalog title, while the central texture was imperfect and
the artist decoded as one unrelated character. The existing 2026-08-17 recording also contains
menu and difficulty overlays whose active-row crops decode unrelated strings. A resolver must
accept the first two without turning the latter into songs.

## Decision

The v1 music-select resolver is
`scorepeek-music-select-active-prefix-corroborated-v1`. The registered production field worker
constructs it from the same complete music-select field observation and full-catalog candidate
domain used by recording and live sessions. Sources do not own alternate resolver calls after the
canonical-frame boundary.

The active-list title is primary evidence. For every catalog song, the candidate domain compares
each admitted observation form with the equally long prefix of each candidate title form. It
retains minimum prefix edit distance and maximum exact integer normalized similarity in addition
to the existing full-title metrics. Candidates are ordered by prefix edit distance, prefix
similarity, and stable `ScorepeekSongId`.

The primary candidate set contains every candidate tied at the minimum prefix edit distance and
meeting all of these conditions:

- the folded active-list comparison key contains at least five Unicode scalar values;
- prefix edit distance is at most one; and
- prefix normalized similarity is at least `6/7`.

Central-title and artist OCR remain one simple crop and one open-text observation each. A
supplemental field is strong only when at least one catalog candidate has full-text edit distance
at most one and normalized similarity at least `4/5`. Weak or empty supplemental evidence is
ignored. Strong evidence cannot overturn a unique active-prefix candidate: if it excludes that
candidate, resolution returns a typed conflict. When the active-prefix set is tied, strong central
and artist candidate sets may narrow it by intersection. Empty intersection is a typed conflict;
more than one survivor remains typed ambiguous. No weighted sum, automatic threshold relaxation,
fallback profile, or guessed song is permitted.

The widened central-title crop covers the independently measured texture extent without adding a
multi-line OCR policy. The active-list context crop is tightened vertically inside the existing
generic list-row slot; generic list-title corpus geometry is unchanged. Observation schema v4
retains the exact OCR values, complete full-title and prefix metrics, selected and runner-up
evidence, corroboration flags, and typed decision/reason in the create-only local recognition
artifact. Foreground NDJSON exposes the matching typed music-select resolution separately from the
existing result resolution. Neither surface grants stable-selection, chart, event, release, or
capture-profile support authority.

## Consequences

- Replaying all 459 canonical frames from the reviewed recording produced 113 field observations
  and 287,924 candidate records. All three result episodes still passed with 22 exact song and
  `CLEAR TYPE` matches. Music-select produced 72 accepted observations covering the four songs
  actually visible during selection and scrolling, while 16 blank, menu, or garbled observations
  remained typed unknown; no unrelated OCR string was accepted.
- The same resolver against the complete 2,548-song active catalog accepted retained `MOVE!` and
  clipped `ASIAN VIRTUAL REALITIES...` active-row observations at prefix edit distance zero with
  runner-up edit margins 15 and 25. Their weak central/artist observations did not participate.
- These observations ground the first fail-closed screen-local music-select resolver. They do not
  establish stable-selection dwell, selected chart, event deduplication, title-disjoint accuracy,
  target-host performance, or support.
