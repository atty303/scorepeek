# ADR 0051: Guide local Gamescope profile setup

- Status: Accepted
- Date: 2026-08-27
- Supersedes: ADR 0047 only for manually transferring a pre-authored capture binding and for its
  operator-started calibration process; ADR 0029/0036 developer admission contracts remain valid

## Context

The cargo-dist executable and separately managed catalog are not sufficient for an ordinary user.
The existing Gamescope path requires a caller to author a binding with rational geometry, retain its
digest, and repeat the complete binding and session provenance on every run. Those are useful
developer gates, but they expose implementation evidence instead of a usable capture-profile
choice.

Scorepeek can create the calibration pixels itself. It can also retain the exact Gamescope argument
vector selected by the operator without inventing a preset matrix or silently treating one
configuration as portable to another machine.

## Decision

`scorepeek setup gamescope --profile NAME [--no-recording] -- GAMESCOPE_ARGS...` starts one
scorepeek-owned calibration Gamescope process around a scorepeek-owned 1920x1080 X11 marker. The
marker has deterministic color cells and five distinct fiducials. Scorepeek acquires the resulting
Gamescope PipeWire source, derives exact aspect-fit rational geometry, normalizes it through the
production normalizer, and accepts it only when the complete marker and every fiducial match the
fixed bounds. Setup owns and terminates only this calibration process; it never starts or signals
INFINITAS.

The resulting canonical binding v2 retains the exact bounded UTF-8 Gamescope argument vector in the
same file as the existing observed contract, provenance, geometry, and normalizer identity. It is
published create-only as `$XDG_CONFIG_HOME/scorepeek/profiles/NAME.json`, falling back to
`$HOME/.config/scorepeek/profiles/NAME.json`. No second profile manifest, enable registry, imported
binding, preset list, or compatibility fallback is added. Arguments that would transfer lifecycle
ownership, including ready-fd, keep-alive, and `-R`, are rejected; other Gamescope options are not
reinterpreted as supported combinations.

`scorepeek profile list` reports the locally valid profiles. `scorepeek run [--profile NAME]
[--no-recording]` selects the named profile, or the only profile when exactly one exists, then uses
the existing foreground capture, diagnostic, and provisional recognition path. It obtains the
active catalog from the normal XDG data store and the fixed model from the common XDG cache. It
rejects a changed installed Gamescope version before source admission. Multiple profiles require an
explicit name. Existing developer commands that accept raw binding paths, digests, and provenance
remain reproducibility gates and are not the ordinary interface.

The setup command is subject to the same global small-model initialization as every non-information
command. The marker child receives the already resolved bundle and does not fetch independently.
`--no-recording` disables both the bounded diagnostic recording and recognition artifact for a
normal run; setup never persists captured calibration pixels. Diagnostic resource binding hashes
the executing binary inode through Linux procfs without adding an identity to the CLI or archive
name. Ordinary recognition artifacts use distinct run directories behind one run lock and refuse a
new recorded run at eight generations or when its reserved maximum would exceed 1 GiB; existing
artifacts are never deleted automatically. Setup success
prints one JSON summary, profile listing prints one JSON array, and routine
recognition keeps its existing NDJSON stdout contract.

## Consequences

- A normal installation needs only the CLI archive, one active catalog, automatic small-model
  cache, and one locally authored profile.
- Profile names replace routine binding/digest/provenance arguments without weakening admission or
  normalizer identity checks.
- Recorded ordinary runs are bounded across runs as well as within each artifact; capacity failure
  leaves existing runs unchanged and can be bypassed explicitly with `--no-recording`.
- New Gamescope versions and materially different output/scaling configurations require a new
  profile; scorepeek neither guesses compatibility nor switches profiles.
- Synthetic tests cover authoring and selection. Creating a real 4K Bazzite profile and qualifying
  it for supported use remain explicit target-machine validation.
