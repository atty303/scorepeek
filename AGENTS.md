# Repository Guidelines

## Authority and scope

- This repository is the source of truth for `scorepeek` design and future
  implementation.
- Do not edit, vendor, merge, subtree, cherry-pick, or import runtime data from
  the upstream `kaktuswald/inf-notebook` repository. It may be consulted once
  as a research hint, but committed layout values must be independently
  measured from scorepeek captures. Upstream code, coordinates, resources,
  catalogs, and generated artifacts are not project inputs.
- The accepted roadmap is `docs/plan.ja.md`. Long-lived decisions live under
  `docs/decisions/`; supersede an ADR with a new ADR instead of rewriting an
  accepted decision.
- The current milestone is capture, recognition, and the versioned event API.
  UI and persistence are out of scope until explicitly requested.

## Checkpoint and resumption

- At task start, take the repository VCS snapshot first, then read `STATUS.md`,
  `docs/plan.ja.md`, and the active ADR index at `docs/decisions/README.md`.
- `STATUS.md` is the single source of truth for the state included in its
  commit. Replace it when updating; do not use it as an append-only log.
- When a logical commit changes the milestone, verified/unverified boundary,
  blocker or required approval, or next executable task, update `STATUS.md` in
  that same commit.
- A dirty working tree is outside the committed checkpoint. Inspect every
  existing change and preserve it; never discard, overwrite, or describe it as
  checkpoint state.
- Use Git history for work history. Keep conversation history, experiments,
  rejected candidates, and trial-and-error details out of `STATUS.md`; record
  only verified facts and the next execution boundary.

## Engineering rules

- Prefer Rust for the runtime. Python is permitted only in reproducible offline
  OCR training and export tooling and must not be a game-session dependency.
- Make invalid states unrepresentable where practical. Recognition must fail
  closed: never guess a field, relax a threshold automatically, or silently
  switch capture profiles.
- Treat Portal, Gamescope direct PipeWire, and an eligible OBS route as opaque
  peer capture profiles; no observable route is a pixel correctness reference.
  Each profile requires its own versioned domain normalizer and independent
  semantic, lifecycle, and performance gates before support.
- Keep canonical recognition input fixed at contiguous RGB8 1920x1080 unless a
  new versioned frame contract is approved.
- Do not import upstream Python modules or read upstream resource formats.
  External catalog adapters must preserve source revision, lineage, provenance,
  and content hashes; they must parse data without executing downloaded code.
- OCR models, dictionaries, and configs require immutable revisions, hashes,
  licenses, and reproducible export records. ADR 0050 permits only the fixed registered
  PP-OCRv6-small bundle to be fetched into the XDG cache during common CLI initialization;
  unregistered runtime downloads, alternate-model selection, and arbitrary local model paths
  remain prohibited.
- Catalog federation must not use fuzzy identity merging, weighted majority, or
  source recency to resolve cross-source conflicts. Ambiguous records stay
  quarantined and the last-known-good catalog remains active.
- Bound every content-addressed runtime store by per-object size, generation
  count, and aggregate bytes. At capacity, existing identical content remains
  usable and new content must fail without changing the active state.
- Treat crash recovery as part of durable activation: recover only
  scorepeek-owned staging entries under the writer lock, and fsync every newly
  created path component and its parent before reporting success.

## Private data and credentials

- In this repository, `private` primarily describes storage, provenance,
  redistribution, and repository-inclusion boundaries; it does not by itself
  mean confidential. Catalog strings and artifact paths or IDs are not secrets
  unless separately identified as credentials, personal or player data, or
  explicitly redacted content. Do not add output suppression, path redaction,
  fixed generic errors, or privacy-review requirements solely because an
  artifact is called private. By default, keep raw external catalog snapshots
  and generated catalogs out of commits and treat source-reuse and
  personal-data boundaries as repository-inclusion exclusions.
- By default, do not commit captured frames, game assets, player/rival data,
  raw external catalog snapshots, generated catalogs, or OCR models. A specific
  artifact may be committed only when the user explicitly approves its
  repository inclusion. For these artifact classes, that explicit instruction
  takes precedence over the default source-reuse, redistribution, and
  personal-data exclusions.
- Never commit credentials, including OBS passwords and tokens, or raw external
  API responses containing secrets.
- By default, store real fixture frames and their complete labels outside the
  repository. Without an approved exception, commit only schemas, opaque
  fixture IDs/hashes, non-personal class labels, independently created and
  redistributable synthetic contract fixtures, explicitly redacted expected
  values, and replay tooling.
- External catalog strings are runtime decoder inputs, not training data.
  Training text must be independently licensed or generated, and real game
  crops and labels remain in the private corpus.
- Respect source-specific access and reuse policy. RemyWiki is reference-only
  unless its administrators grant explicit permission for automated reuse.

## Trust boundaries

- Treat private artifacts that the operator explicitly creates or selects as
  trusted inputs. Validate only what detects ordinary mistakes at the boundary:
  declared digest, required schema/fields, and invariants needed by the
  requested result. Do not add per-record cross-artifact re-adjudication,
  adversarial substitution defenses, or equivalent repeated full reads solely
  to defend against that operator.
- Keep validation for untrusted or mutable boundaries: network/catalog bytes,
  remote storage, concurrent writers, content-addressed stores, filesystem
  traversal, crash recovery, and a runtime's own acceptance contracts. Remove
  an existing trusted-input check when its cost is material and it provides no
  ordinary-mistake detection or one of these boundary guarantees.

## Tooling and verification

- Expose normal repository operations through `mise`.
- `mise run check` is non-mutating, `mise run fix` applies supported fixes, and
  `mise run test` is the complete reproducible validation entry point.
- Define fast checks once in `hk.pkl`; hooks and mise tasks must reuse them.
- Keep live Bazzite/Portal/OBS/Gamescope/GPU tests as explicit tasks. Never
  represent development-host or synthetic success as target-machine
  validation.
- Before declaring a capture backend supported, satisfy its target performance,
  lifecycle, and recognition gates from the plan.

## Version control

- Preserve unrelated user work and commit only the current logical change.
- Do not create a remote, push, publish, release, or change external services
  unless the user explicitly requests it.
