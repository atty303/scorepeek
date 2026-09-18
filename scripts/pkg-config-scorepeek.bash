#!/usr/bin/env bash
set -euo pipefail

route=
package_requirement_pattern='^(libpipewire-0[.]3|libspa-0[.]2|xkbcommon|openssl)([[:space:]]*(<=|>=|=|<|>)[[:space:]]*[[:graph:]]+)?$'
for argument in "$@"; do
  case "$argument" in
    -*) continue ;;
    *)
      if [[ ! "$argument" =~ $package_requirement_pattern ]]; then
        echo "unsupported pkg-config package: $argument" >&2
        exit 2
      fi
      package="${BASH_REMATCH[1]}"
      ;;
  esac

  case "$package" in
    libpipewire-0.3 | libspa-0.2) requested_route=pipewire ;;
    xkbcommon) requested_route=xkbcommon ;;
    openssl) requested_route=system ;;
    *)
      echo "unsupported pkg-config package: $argument" >&2
      exit 2
      ;;
  esac

  if [[ -n "$route" && "$route" != "$requested_route" ]]; then
    echo "pkg-config packages require different SDK routes" >&2
    exit 2
  fi
  route="$requested_route"
done

case "$route" in
  pipewire) sdk_root="$(mise where 'http:libpipewire-sdk@1.6.8-1')" ;;
  xkbcommon) sdk_root="$(mise where 'http:libxkbcommon-sdk@1.7.0-2')" ;;
  system) exec pkg-config "$@" ;;
  *)
    echo "pkg-config query did not name a supported package" >&2
    exit 2
    ;;
esac
pkgconf_root="$(mise where 'http:pkgconf@3.0.1.post0')"

export PKG_CONFIG_DIR=
export PKG_CONFIG_PATH=
export PKG_CONFIG_LIBDIR="$sdk_root/usr/lib/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$sdk_root"

exec "$pkgconf_root/pkgconf/.bin/pkgconf" "$@"
