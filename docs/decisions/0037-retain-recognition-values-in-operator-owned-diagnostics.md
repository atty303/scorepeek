# ADR 0037: Retain recognition values in operator-owned diagnostics

- Status: Accepted
- Date: 2026-08-24
- Supersedes: ADR 0032, ADR 0035, and ADR 0036 only where they exclude recognition
  values from operator-owned local diagnostics or simulation artifacts
- Complements: ADR 0034's complete full-catalog candidate evidence

## Context

The field-observation and recording-simulation gates were designed to prove execution and
lifecycle boundaries before score acceptance existed. Their small command results intentionally
reported counts and digests, but the same design also excluded OCR strings, song IDs, and candidate
scores from local diagnostic evidence.

That exclusion is not justified by the current data-handling contract. OCR text rendered by the
game, catalog strings, `ScorepeekSongId` values, comparison metrics, artifact paths, and digests are
not credentials and are not confidential merely because a run or artifact is private. Suppressing
them prevents an operator from checking what the recognizer observed, why one candidate won or was
rejected, and whether a recording simulation reproduced its reviewed labels. Counts alone can
prove that code ran, but cannot prove recognition correctness.

Captured pixels, game assets, player or rival data, and complete private labels remain subject to
the repository-inclusion and authorized-use boundaries in `AGENTS.md`. Raw credential values
remain excluded from diagnostic, result, and event artifacts, display, and commits. Those
boundaries do not require recognition values to be removed from an operator-owned local diagnostic
artifact.

## Decision

Operator-owned local recognition diagnostics and simulation artifacts actively retain the exact
bounded values needed to reproduce and review a recognition decision. As the corresponding stages
are implemented, this evidence includes:

- every observed field state and exact OCR string, including `CLEAR TYPE`;
- a run-scoped table containing every exact catalog display and comparison string used by the
  bound active catalog, stored once, plus an unambiguous reference from each candidate to its
  string and comparison-form identity;
- the complete per-field metrics for every song in the bound active catalog;
- each `ScorepeekSongId`, the selected candidate and runner-up, margins, and the typed reason for
  acceptance, rejection, or unknown;
- the temporal episode and selection-context inputs and decisions used by the resolver;
- reviewed expected versus observed song IDs, field values, and episode outcomes in simulation;
  and
- immutable run, resource, source-frame sequence, and presentation-timestamp provenance needed to
  join that evidence to its inputs.

These values are primary recognition evidence, not arbitrary diagnostic properties. Their schemas
are allowlisted and versioned. Existing bounds on OCR output, fields, active-catalog size, frames,
episodes, diagnostic generations, per-object bytes, and aggregate bytes must also bound the stored
evidence. Capacity loss or disabled recording is reported as diagnostic degradation and never
changes the recognition result. Pixel artifacts remain in their existing bounded image store and
are referenced by identity rather than duplicated inside value records.

A compact command result may still expose only typed status, recognition decisions, counts,
digests, and artifact identity when that is sufficient for automation. That compactness is a result
contract, not a secrecy or privacy guarantee. The referenced operator-owned artifact must contain
the exact recognition evidence above. A command whose purpose is interactive recognition review
may also return the recognized values directly.

No value is suppressed solely because it is private, high entropy, environment-specific, a path,
a digest, an OCR string, or a song ID. Values are minimized only for a distinct sink with a stated
purpose, such as the versioned event API, or because an existing repository-inclusion,
confidentiality, player-data, redistribution, or credential boundary requires it. Publishing or
sending a local artifact outside the operator-controlled environment remains a separate explicit
action.

ADR 0034 remains authoritative that full-catalog scoring itself does not rank or accept a song. Its
pure function need not perform recording side effects; the application-owned recognition run
records its returned evidence.

## Consequences

- The completed recording field simulation proves the shared field and scoring path executed, but
  its counts-only output does not prove song recognition correctness.
- The next recognition checkpoint must add a bounded value-bearing artifact before selecting or
  tuning resolver policy, then replay the private recording and review expected versus observed
  song and result values.
- Live INFINITAS Gamescope recognition remains blocked until that simulation recognition path is
  complete and independently reviewed.
- The future event API may intentionally omit raw OCR and candidate lists without requiring the
  local diagnostic artifact to omit them.
