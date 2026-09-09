#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
xdg=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skins.XXXXXX")
cleanup() {
  rm -rf "$xdg"
}
trap cleanup EXIT INT TERM

"$root/scripts/build-skins.sh"
cargo build --locked -p scorepeek

export XDG_DATA_HOME="$xdg/data"
export XDG_CONFIG_HOME="$xdg/config"
export XDG_CACHE_HOME="$xdg/cache"
if [[ "${SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR:-0}" != 1 ]]; then
  export XDG_RUNTIME_DIR="$xdg/runtime"
  mkdir -p "$XDG_RUNTIME_DIR"
  chmod 700 "$XDG_RUNTIME_DIR"
fi

for package in "$root"/target/skins/*.zip; do
  "$root/target/debug/scorepeek" skin install "$package"
done

"$@"
