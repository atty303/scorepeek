# ADR 0053: Follow operator-owned local symlinks

- Status: Accepted
- Date: 2026-08-28
- Supersedes: ADR 0014 where it treats the absence of symlinks as a validity condition for
  operator-selected local paths; earlier no-symlink requirements remain only for owned recovery,
  deletion, and create-only publication entries

## Context

Scorepeek rejected a path whenever any existing component was a symbolic link. On Bazzite,
`/home` is normally a link to `/var/home`, so the ordinary fallback
`$HOME/.config/scorepeek/profiles` failed before guided Gamescope setup could create a profile.
The same assumption existed at several XDG state, cache, catalog, diagnostic, model, profile, and
local artifact boundaries.

These paths are selected and managed by the same operator who runs scorepeek. A symlink there is
normal filesystem organization, not a separate trust domain. Required file type, size, digest,
schema, and runtime admission checks already detect the ordinary mistakes that matter to the
result.

## Decision

Scorepeek and its offline corpus tooling follow symbolic links in operator-selected local roots,
their ancestors, and read-only local inputs. Validation applies to the resolved file or directory:
required type, bounded size, registered digest, schema, and semantic invariants are unchanged.
Paths do not need to be lexically canonical, and a symlink is not rejected solely because it is a
symlink.

Create-only publication still treats any existing destination directory entry, including a
symlink, as occupied and does not overwrite it. Automatic crash recovery and deletion inspect the
directory entries they own without following a substituted symlink target; an entry that cannot be
proved to be the expected scorepeek-owned staging or deletion object is preserved and the operation
fails closed. Archive traversal and network-input validation are unchanged because they cross a
different control boundary.

The offline `scorepeek-corpus` content-addressed store applies the same rule to operator roots,
source locators, and content reads. Its mutation and recovery steps still do not follow a symlinked
entry when selecting something to overwrite or delete.

## Consequences

- Standard Bazzite `/home -> /var/home` layouts work with the documented HOME-based XDG fallbacks.
- Operators may place config, state, cache, catalogs, models, profiles, diagnostics, and local
  read-only artifacts behind symlinks without weakening their content contracts.
- Scorepeek does not attempt to decide whether an operator's local symlink target is trustworthy.
- No-clobber publication and owned cleanup remain non-following where following could overwrite or
  delete a different object.
