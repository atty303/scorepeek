#!/usr/bin/env bash
set -euo pipefail

sdk_root="$(mise where 'http:libpipewire-sdk@1.6.8-1')"
pkgconf_root="$(mise where 'http:pkgconf@3.0.1.post0')"
pkgconf="$pkgconf_root/pkgconf/.bin/pkgconf"

required_paths=(
  "$sdk_root/usr/include/pipewire-0.3/pipewire/pipewire.h"
  "$sdk_root/usr/include/spa-0.2/spa/param/video/raw.h"
  "$sdk_root/usr/lib/libpipewire-0.3.so"
  "$sdk_root/usr/lib/libpipewire-0.3.so.0"
  "$sdk_root/usr/lib/pkgconfig/libpipewire-0.3.pc"
  "$sdk_root/usr/lib/pkgconfig/libspa-0.2.pc"
  "$sdk_root/usr/share/licenses/libpipewire/COPYING"
  "$pkgconf"
  "$pkgconf_root/pkgconf-3.0.1.post0.dist-info/licenses/LICENSE"
)

for required_path in "${required_paths[@]}"; do
  if [[ ! -e "$required_path" ]]; then
    echo "native build prerequisite is missing: $required_path" >&2
    exit 2
  fi
done

if ! command -v cc >/dev/null 2>&1; then
  echo "native build prerequisite is missing: C compiler 'cc'" >&2
  exit 2
fi

libclang="$(scripts/resolve-clang-build-input.bash library)"
clang_resource_dir="$(scripts/resolve-clang-build-input.bash resource-dir)"
if [[ "${LIBCLANG_PATH:-}" != "$libclang" ]]; then
  echo "mise did not fix LIBCLANG_PATH to the verified shared library" >&2
  exit 2
fi

version="$(scripts/pkg-config-scorepeek.bash --modversion libpipewire-0.3)"
if [[ "$version" != "1.6.8" ]]; then
  echo "unexpected libpipewire SDK version: $version" >&2
  exit 2
fi

flags="$(scripts/pkg-config-scorepeek.bash --cflags --libs libpipewire-0.3 libspa-0.2)"
if [[ "$flags" != *"$sdk_root/usr/include/pipewire-0.3"* ]] \
  || [[ "$flags" != *"$sdk_root/usr/include/spa-0.2"* ]] \
  || [[ "$flags" != *"$sdk_root/usr/lib"* ]] \
  || [[ "$flags" != *"-lpipewire-0.3"* ]]; then
  echo "pkgconf did not resolve the complete pinned PipeWire SDK" >&2
  exit 2
fi

if [[ "$flags" == *" -I/usr/"* || "$flags" == *" -L/usr/"* ]]; then
  echo "pkgconf leaked host development paths into the pinned SDK" >&2
  exit 2
fi

probe_root="$(mktemp -d)"
cleanup() {
  rm -rf -- "$probe_root"
}
trap cleanup EXIT

probe_source="$probe_root/pipewire-runtime.c"
probe_binary="$probe_root/pipewire-runtime"
printf '%s\n' \
  '#include <stdio.h>' \
  '#include <pipewire/pipewire.h>' \
  'int main(void) {' \
  '  const char *version = pw_get_library_version();' \
  '  if (version == NULL || version[0] == '\''\0'\'') return 2;' \
  '  puts(version);' \
  '  return 0;' \
  '}' >"$probe_source"
read -r -a pipewire_flags <<<"$flags"
cc "$probe_source" "${pipewire_flags[@]}" -o "$probe_binary"
host_pipewire_version="$(env -u LD_LIBRARY_PATH -u LD_PRELOAD "$probe_binary")"
if [[ -z "$host_pipewire_version" ]]; then
  echo "host PipeWire runtime returned an empty version" >&2
  exit 2
fi

printf 'pipewire_sdk_version=%s\n' "$version"
printf 'pipewire_sdk_root=%s\n' "$sdk_root"
printf 'pkgconf_version=%s\n' "$("$pkgconf" --version)"
compiler_version="$(cc --version)"
printf 'c_compiler=%s\n' "${compiler_version%%$'\n'*}"
printf 'libclang=%s\n' "$libclang"
printf 'clang_resource_dir=%s\n' "$clang_resource_dir"
printf 'host_pipewire_version=%s\n' "$host_pipewire_version"
