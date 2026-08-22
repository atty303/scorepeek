# ADR 0028: Build PipeWire against a mise-pinned SDK

- Status: accepted
- Date: 2026-08-22

## Context

ADR 0027 requires safe Rust PipeWire bindings and native libpipewire/libspa
headers. Installing all development packages on every host would make the
build depend on mutable distro repositories. Building the whole repository in
a container would make that environment more reproducible, but would also move
the normal Cargo edit/check/test loop away from the host. The operator wants
host-native Cargo commands with mise as the only project bootstrap tool.

The Rust bindings generate FFI bindings at build time. That requires a C
compiler and libclang with its matching Clang resource headers. It does not
require Zig. `pkg-config` is also needed during the build, but adding a Python
runtime solely to obtain a compatible implementation is not justified.

## Decision

The first PipeWire build environment supports Linux x86-64. The workspace uses
the safe `pipewire` 0.10 series and commits the exact resolved dependency graph
in `Cargo.lock`.

Mise installs two immutable, checksum-pinned build inputs:

- the Arch Linux Archive `libpipewire` 1.6.8-1 package, used only as the
  MIT-licensed libpipewire/libspa SDK; and
- the native `pkgconf` 3.0.1 executable extracted directly from its manylinux
  MIT-licensed wheel without invoking or requiring Python.

The selected `pipewire` Rust crate is MIT-licensed. Both downloaded archives
retain their upstream license files, and the native verification checks those
files together with the executable, headers, libraries, and pkg-config data.

A repository wrapper clears ambient pkg-config search paths and exposes only
the pinned SDK metadata and sysroot. Mise also fixes both bindgen's exact
x86-64 shared-libclang file and the same-major Clang resource directory. The
lookup is read-only, bounded to the host loader inventory and standard host
Clang resource locations, ignores ambient `LIBCLANG_PATH`, and fails when the
shared library and resource-header major versions do not match.

The host remains responsible for:

- a native C compiler available as `cc`;
- a shared libclang and matching Clang resource headers; and
- the normal PipeWire runtime library used when the resulting binary runs.

The Clang driver, Zig, host `pkg-config`, Podman, Distrobox, and the operator's
personal distrobox image are not project build dependencies. `mise run
native:verify` checks the pinned artifacts and host boundary before Rust
compilation. It also links a minimal probe against the pinned SDK and executes
it with the default host loader to prove that the runtime PipeWire ABI is
available. The complete reproducible test entry point runs that check.

## Consequences

- Developers keep the ordinary host-native Cargo workflow after `mise
  install` and mise activation.
- The large and mutable SDK surface is content-pinned independently of the
  host distro, while the small compiler/runtime ABI boundary stays a declared
  host prerequisite.
- The Python-shaped distribution archive for pkgconf does not add Python to
  the build or runtime graph; only its native executable is used.
- This bootstrap does not make another architecture or operating system
  supported. A new target needs its own SDK/pkgconf artifacts, checksums, and
  native build verification.
- Successful compilation does not establish a working or supported Gamescope
  capture profile; ADR 0027's live semantic, lifecycle, and performance gates
  remain required.
