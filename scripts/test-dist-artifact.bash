#!/usr/bin/env bash
set -euo pipefail

if (( $# > 1 )); then
  echo 'usage: scripts/test-dist-artifact.bash [VERSION]' >&2
  exit 2
fi

readonly version="${1:-0.1.0}"
readonly target="x86_64-unknown-linux-gnu"
readonly archive="target/distrib/scorepeek-${target}.tar.xz"
readonly legacy_checksum="${archive}.sha256"
readonly work_dir="$(mktemp -d)"

restore_source() {
  if [[ -f "$work_dir/scorepeek-Cargo.toml" ]]; then
    cp -p -- "$work_dir/scorepeek-Cargo.toml" crates/scorepeek/Cargo.toml
    cp -p -- "$work_dir/Cargo.lock" Cargo.lock
  fi
  rm -rf -- "$work_dir"
}
trap restore_source EXIT

dist_args=(build --artifacts=local)
rm -f -- "$legacy_checksum"
if (( $# == 1 )); then
  if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf 'release version must contain three numeric components: %s\n' "$version" >&2
    exit 2
  fi
  cp -p -- crates/scorepeek/Cargo.toml "$work_dir/scorepeek-Cargo.toml"
  cp -p -- Cargo.lock "$work_dir/Cargo.lock"
  sed -i "0,/^version = \".*\"$/s//version = \"${version}\"/" crates/scorepeek/Cargo.toml
  cargo metadata --format-version=1 --no-deps >/dev/null
  dist_args+=(--tag "v${version}")
fi
dist "${dist_args[@]}"

test -f "$archive"
test ! -e "$legacy_checksum"

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

tar -xJf "$archive" -C "$work_dir"

readonly root="$work_dir/scorepeek-${target}"
readonly binary="$root/scorepeek"
SCOREPEEK_TEST_BINARY="$binary" cargo test --locked -p scorepeek --features embedded-web --test overlay
mkdir -p "$work_dir/home" "$work_dir/data" "$work_dir/cache"

run_scorepeek() {
  env -i \
    HOME="$work_dir/home" \
    XDG_DATA_HOME="$work_dir/data" \
    XDG_CACHE_HOME="$work_dir/cache" \
    PATH=/usr/bin:/bin \
    "$binary" "$@"
}

version_output="$($binary --version)"
if [[ "$version_output" != "scorepeek $version" ]]; then
  printf 'archive version mismatch: expected %q, got %q\n' "scorepeek $version" "$version_output" >&2
  exit 1
fi

doctor_output="$(run_scorepeek doctor --format json)"
case "$doctor_output" in
  *'"schema":"scorepeek-target-inventory-v1"'*'"status":"not_installed"'*) ;;
  *)
    printf 'doctor returned an unexpected payload: %s\n' "$doctor_output" >&2
    exit 1
    ;;
esac

test "$(run_scorepeek vulkan-layer install)" = "installed"
test "$(run_scorepeek vulkan-layer install)" = "unchanged"
doctor_output="$(run_scorepeek doctor --format json)"
case "$doctor_output" in
  *'"status":"matches_embedded"'*) ;;
  *)
    printf 'doctor did not report the installed layer: %s\n' "$doctor_output" >&2
    exit 1
    ;;
esac

readonly manifest="$work_dir/data/vulkan/explicit_layer.d/VkLayer_SCOREPEEK_capture.json"
readonly layer="$work_dir/data/scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so"
grep -q '"library_path": "../../scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so"' "$manifest"
! readelf -S "$layer" | grep -q '\.symtab'
readelf -h "$layer" | grep -Eq 'Machine:.*X86-64'
! objdump -d "$layer" | grep -Eq '%[yz]mm|[[:space:]]v[a-z][a-z0-9]+'

printf 'different payload' >"$layer"
doctor_output="$(run_scorepeek doctor --format json)"
case "$doctor_output" in
  *'"status":"different_payload"'*) ;;
  *)
    printf 'doctor did not report the changed layer: %s\n' "$doctor_output" >&2
    exit 1
    ;;
esac
test "$(run_scorepeek vulkan-layer install)" = "updated"
test "$(run_scorepeek vulkan-layer uninstall)" = "uninstalled"
test ! -e "$manifest"
test ! -e "$layer"
test -d "$work_dir/data/vulkan/explicit_layer.d"
test "$(run_scorepeek vulkan-layer uninstall)" = "not installed"
