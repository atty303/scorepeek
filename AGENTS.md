# Repository Guidelines

## Authority and scope

- This repository is the source of truth for `scorepeek` design and future
  implementation.
- Do not edit, vendor, merge, subtree, or cherry-pick the upstream
  `kaktuswald/inf-notebook` repository. Upstream release tags are external
  inputs consumed only through the planned adoption workflow.
- The accepted roadmap is `docs/plan.ja.md`. Long-lived decisions live under
  `docs/decisions/`; supersede an ADR with a new ADR instead of rewriting an
  accepted decision.
- The current milestone is capture, recognition, and the versioned event API.
  UI and persistence are out of scope until explicitly requested.

## Engineering rules

- Prefer Rust for the runtime. Python is permitted only in the isolated
  adoption-time resource importer and must not be a game-session dependency.
- Make invalid states unrepresentable where practical. Recognition must fail
  closed: never guess a field, relax a threshold automatically, or silently
  switch capture profiles.
- OBS WebSocket PNG and Gamescope PipeWire are distinct frame sources with
  separate capture and recognition profile IDs, fixtures, and release gates.
- Keep canonical recognition input fixed at contiguous RGB8 1920x1080 unless a
  new versioned frame contract is approved.
- Do not import upstream Python modules at runtime. Never unpickle an upstream
  resource until its filename and SHA-256 match a human-approved manifest that
  existed before the importer started. A digest calculated from the same
  unapproved download is not a trust anchor.
- Apply the same pre-approval rule to OCR models, dictionaries, and configs.
  Runtime model auto-download and arbitrary unregistered local model paths are
  prohibited.

## Private data and credentials

- Never commit captured frames, game assets, player/rival data, converted
  upstream resources, OCR models, OBS passwords, tokens, or raw external API
  responses containing secrets.
- Store real fixture frames and their complete labels outside the repository.
  Commit only schemas, opaque fixture IDs/hashes, non-personal class labels,
  independently created synthetic contract fixtures, explicitly redacted
  expected values, and replay tooling.
- OBS WebSocket access is localhost-only and authenticated. Credentials must
  not appear in normal config, command arguments, logs, events, or test output.

## Tooling and verification

- Expose normal repository operations through `mise`.
- `mise run check` is non-mutating, `mise run fix` applies supported fixes, and
  `mise run test` is the complete reproducible validation entry point.
- Define fast checks once in `hk.pkl`; hooks and mise tasks must reuse them.
- Keep live Bazzite/OBS/Gamescope/GPU tests as explicit tasks. Never represent
  development-host or synthetic success as target-machine validation.
- Before declaring a capture backend supported, satisfy its target performance,
  lifecycle, and recognition gates from the plan.

## Version control

- Preserve unrelated user work and commit only the current logical change.
- Do not create a remote, push, publish, release, or change external services
  unless the user explicitly requests it.
