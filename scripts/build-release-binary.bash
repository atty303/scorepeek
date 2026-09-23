#!/usr/bin/env bash
set -euo pipefail

if (( $# != 2 )); then
  echo 'usage: mise run release:build -- VERSION OUTPUT_DIRECTORY' >&2
  exit 2
fi

readonly version="$1"
readonly output_directory="$2"
readonly target="x86_64-unknown-linux-gnu"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'release version must contain three numeric components: %s\n' "$version" >&2
  exit 2
fi
if [[ ! -d "$output_directory" ]]; then
  printf 'release output directory does not exist: %s\n' "$output_directory" >&2
  exit 2
fi

readonly work_dir="$(mktemp -d)"
restore_source() {
  local status=$?
  if [[ -f "$work_dir/scorepeek-Cargo.toml" ]]; then
    cp -p -- "$work_dir/scorepeek-Cargo.toml" crates/scorepeek-cli/Cargo.toml
  fi
  if [[ -f "$work_dir/Cargo.lock" ]]; then
    cp -p -- "$work_dir/Cargo.lock" Cargo.lock
  fi
  rm -rf -- "$work_dir"
  return "$status"
}
trap restore_source EXIT
cp -p -- crates/scorepeek-cli/Cargo.toml "$work_dir/scorepeek-Cargo.toml"
cp -p -- Cargo.lock "$work_dir/Cargo.lock"

sed -i "0,/^version = \".*\"$/s//version = \"${version}\"/" crates/scorepeek-cli/Cargo.toml
cargo update --offline --workspace
cargo build --locked --profile dist --target "$target" -p scorepeek-cli --bin scorepeek

readonly binary="target/${target}/dist/scorepeek"
readonly actual_version="$("$binary" --version)"
if [[ "$actual_version" != "scorepeek $version" ]]; then
  printf 'release version mismatch: expected %q, got %q\n' "scorepeek $version" "$actual_version" >&2
  exit 1
fi
cp -- "$binary" "$output_directory/scorepeek-${version}-${target}"
