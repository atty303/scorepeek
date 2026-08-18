# ADR 0014: Delegate local filesystem permissions to the operator

- Status: accepted
- Date: 2026-08-18
- Supersedes: ADR 0010 and ADR 0012 only where they require or validate local
  filesystem modes

## Context

Scorepeek previously treated exact Unix modes such as `0700` and `0600`, and
the absence of group/world write bits, as part of corpus, dataset, catalog,
media-output, and model-store validity. That made otherwise valid
content-addressed data unusable on operator-selected filesystems, shared
development paths, ACL-managed storage, and ordinary temporary directories.
It also mixed byte integrity with a deployment policy that scorepeek cannot
fully determine from mode bits alone.

## Decision

Local filesystem access control is an operator responsibility. Scorepeek does
not reject an existing directory or regular file solely because of its Unix
mode, ownership, group, ACL, mount policy, or group/world writability, and it
does not rewrite existing modes during reuse.

Scorepeek continues to reject symlinks at managed boundaries, enforce expected
file and directory types, bind complete bytes by size and SHA-256, publish with
no-clobber semantics, recover only owned staging entries, and preserve fsync
boundaries. These integrity and ownership-marker checks do not imply
confidentiality.

New files and directories may request restrictive creation modes where the
platform API already supports doing so. Those values are best-effort defaults,
not accepted-input criteria, durable-output guarantees, schema fields, or
content identities. The operator must configure permissions, ownership, ACLs,
mount options, backups, and retention appropriate for captured media, labels,
catalog caches, and model artifacts.

This decision changes only local filesystem permission handling. Remote
credentials, TLS, private bucket access, bounded transfer, and secret-safe
error requirements remain unchanged.

## Consequences

- A valid store or output parent can be used from an operator-chosen path such
  as `/tmp` without first changing it to an exact mode.
- External recordings remain admissible when group/world writable; their
  canonical path, regular-file type, size, and complete digest still fail
  closed.
- Scorepeek no longer promises that created artifacts are confidential by
  virtue of their mode. Operators must secure every local storage root they
  select.
