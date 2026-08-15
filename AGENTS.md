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
- Treat Wayland Portal as the post-scale correctness reference. Gamescope
  direct PipeWire and a proven post-scale OBS source are target-machine
  candidates with separate capture profile IDs and release gates. A native FHD
  game source is pre-scale and cannot satisfy the canonical capture contract.
- Keep canonical recognition input fixed at contiguous RGB8 1920x1080 unless a
  new versioned frame contract is approved.
- Do not import upstream Python modules or read upstream resource formats.
  External catalog adapters must preserve source revision, lineage, provenance,
  and content hashes; they must parse data without executing downloaded code.
- OCR models, dictionaries, and configs require immutable revisions, hashes,
  licenses, and reproducible export records. Runtime model auto-download and
  arbitrary unregistered local model paths are prohibited.
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

- Never commit captured frames, game assets, player/rival data, raw external
  catalog snapshots, generated catalogs, OCR models, OBS passwords, tokens, or
  raw external API responses containing secrets.
- Store real fixture frames and their complete labels outside the repository.
  Commit only schemas, opaque fixture IDs/hashes, non-personal class labels,
  independently created and redistributable synthetic contract fixtures,
  explicitly redacted expected values, and replay tooling.
- External catalog strings are runtime decoder inputs, not training data.
  Training text must be independently licensed or generated, and real game
  crops and labels remain in the private corpus.
- Respect source-specific access and reuse policy. RemyWiki is reference-only
  unless its administrators grant explicit permission for automated reuse.

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
