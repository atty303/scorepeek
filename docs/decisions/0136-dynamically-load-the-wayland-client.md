# ADR 0136: Dynamically load the Wayland client

- Status: Accepted
- Date: 2026-09-06

## Decision

The native overlay shell enables `wayland-client`'s `system` and `dlopen` features directly. The
system backend exposes the raw display and surface pointers required by the renderer; dynamic loading
keeps that backend without running its pkg-config/link probe. Building scorepeek must not require a
host `wayland-client.pc`, Wayland development headers, or an unversioned linker name. The live
Wayland route loads the normal versioned `libwayland-client.so.0` supplied by the target host at
runtime.

The complete repository test entry point checks `scorepeek-overlay-handles` as a standalone package
under the repository pkg-config boundary. This check must not rely on feature unification from the
renderer or another workspace member to enable dynamic loading.

## Consequences

Wayland development packages are no longer an undeclared build input. The pinned PipeWire SDK,
pkgconf wrapper, compiler and libclang boundaries accepted by ADR 0028 are unchanged. A machine may
build and test the Wayland shell without a running compositor, but exercising the live backend still
requires a Wayland session and a compatible runtime client library and remains an explicit target
validation boundary.

## Verification

`mise run overlay:wayland:build:check` builds, links and tests the shell crate independently while
the repository pkg-config wrapper exposes only the pinned PipeWire SDK. `mise run test` includes
this check. A live Wayland run separately verifies runtime library loading, compositor protocols,
rendering and input.
