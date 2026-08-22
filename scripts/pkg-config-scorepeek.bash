#!/usr/bin/env bash
set -euo pipefail

sdk_root="$(mise where 'http:libpipewire-sdk@1.6.8-1')"
pkgconf_root="$(mise where 'http:pkgconf@3.0.1.post0')"

export PKG_CONFIG_DIR=
export PKG_CONFIG_PATH=
export PKG_CONFIG_LIBDIR="$sdk_root/usr/lib/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$sdk_root"

exec "$pkgconf_root/pkgconf/.bin/pkgconf" "$@"
