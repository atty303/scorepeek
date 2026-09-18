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
if ! command -v pkg-config >/dev/null 2>&1; then
  echo "native build prerequisite is missing: host 'pkg-config'" >&2
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

xkbcommon_version="$(scripts/pkg-config-scorepeek.bash --modversion xkbcommon)"
if [[ "$xkbcommon_version" != "1.7.0" ]]; then
  echo "unexpected xkbcommon SDK version: $xkbcommon_version" >&2
  exit 2
fi

system_openssl_version="$(pkg-config --modversion openssl)"
wrapped_openssl_version="$(scripts/pkg-config-scorepeek.bash --modversion openssl)"
if [[ "$wrapped_openssl_version" != "$system_openssl_version" ]]; then
  echo "pkg-config wrapper did not resolve the host OpenSSL package" >&2
  exit 2
fi
scripts/pkg-config-scorepeek.bash --exists 'openssl >= 1.0.0'

if mixed_error="$(scripts/pkg-config-scorepeek.bash --exists 'openssl >= 1.0.0' 'libpipewire-0.3 >= 0.3' 2>&1)"; then
  echo "pkg-config wrapper accepted packages from different SDK routes" >&2
  exit 2
fi
if [[ "$mixed_error" != "pkg-config packages require different SDK routes" ]]; then
  echo "pkg-config wrapper returned an unexpected mixed-route error: $mixed_error" >&2
  exit 2
fi

unsupported_queries=(
  'scorepeek-unsupported-probe'
  'openssl zlib'
  'openssl >= 1.0.0 libpipewire-0.3'
)
for unsupported_query in "${unsupported_queries[@]}"; do
  if unsupported_error="$(scripts/pkg-config-scorepeek.bash --exists "$unsupported_query" 2>&1)"; then
    echo "pkg-config wrapper accepted an unsupported package requirement: $unsupported_query" >&2
    exit 2
  fi
  if [[ "$unsupported_error" != "unsupported pkg-config package: $unsupported_query" ]]; then
    echo "pkg-config wrapper returned an unexpected unsupported-package error: $unsupported_error" >&2
    exit 2
  fi
done

flags="$(scripts/pkg-config-scorepeek.bash --cflags --libs libpipewire-0.3 libspa-0.2)"
if [[ "$flags" != *"$sdk_root/usr/include/pipewire-0.3"* ]] \
  || [[ "$flags" != *"$sdk_root/usr/include/spa-0.2"* ]] \
  || [[ "$flags" != *"$sdk_root/usr/lib"* ]] \
  || [[ "$flags" != *"-lpipewire-0.3"* ]]; then
  echo "pkgconf did not resolve the complete pinned PipeWire SDK" >&2
  exit 2
fi

qualified_flags="$(scripts/pkg-config-scorepeek.bash --cflags --libs 'libpipewire-0.3 >= 0.3' 'libspa-0.2 >= 0.2')"
if [[ "$qualified_flags" != "$flags" ]]; then
  echo "version-qualified PipeWire query did not resolve the pinned SDK" >&2
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
printf 'xkbcommon_sdk_version=%s\n' "$xkbcommon_version"
printf 'host_openssl_version=%s\n' "$wrapped_openssl_version"
printf 'pipewire_sdk_root=%s\n' "$sdk_root"
printf 'pkgconf_version=%s\n' "$("$pkgconf" --version)"
compiler_version="$(cc --version)"
printf 'c_compiler=%s\n' "${compiler_version%%$'\n'*}"
printf 'libclang=%s\n' "$libclang"
printf 'clang_resource_dir=%s\n' "$clang_resource_dir"
printf 'host_pipewire_version=%s\n' "$host_pipewire_version"
