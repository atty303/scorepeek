#!/usr/bin/env bash
set -euo pipefail

readonly version="0.1.0"
readonly target="x86_64-unknown-linux-gnu"
readonly archive="target/distrib/scorepeek-${target}.tar.xz"
readonly checksum="${archive}.sha256"

dist build

test -f "$archive"
test -f "$checksum"
(cd "$(dirname "$archive")" && sha256sum --check "$(basename "$checksum")")

mapfile -t members < <(tar -tJf "$archive" | sed '/\/$/d' | sort)
expected_members=(
  "scorepeek-${target}/README.md"
  "scorepeek-${target}/scorepeek"
)
if [[ "${members[*]}" != "${expected_members[*]}" ]]; then
  printf 'unexpected archive contents:\n' >&2
  printf '  %s\n' "${members[@]}" >&2
  exit 1
fi

work_dir="$(mktemp -d)"
trap 'rm -rf -- "$work_dir"' EXIT
tar -xJf "$archive" -C "$work_dir"

readonly root="$work_dir/scorepeek-${target}"
readonly binary="$root/scorepeek"
SCOREPEEK_TEST_BINARY="$binary" cargo test --locked -p scorepeek --features embedded-web --test overlay
mkdir -p "$work_dir/home" "$work_dir/data" "$work_dir/cache"

version_output="$($binary --version)"
test "$version_output" = "scorepeek $version"

doctor_output="$(env -i \
  HOME="$work_dir/home" \
  XDG_DATA_HOME="$work_dir/data" \
  XDG_CACHE_HOME="$work_dir/cache" \
  PATH=/usr/bin:/bin \
  "$binary" doctor)"
case "$doctor_output" in
  *'"schema":"scorepeek-target-inventory-v1"'*) ;;
  *)
    printf 'doctor returned an unexpected payload: %s\n' "$doctor_output" >&2
    exit 1
    ;;
esac
