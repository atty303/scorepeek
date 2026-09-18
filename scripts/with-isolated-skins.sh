#!/usr/bin/env bash
set -euo pipefail

if [[ "${SCOREPEEK_ISOLATED_SKINS:-0}" == 1 ]]; then
  exec "$@"
fi

root=$(cd "$(dirname "$0")/.." && pwd)
xdg=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skins.XXXXXX")
export SCOREPEEK_ISOLATED_SKINS=1
export MISE_DATA_DIR="${MISE_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mise}"
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/ms-playwright}"
cleanup() {
  rm -rf "$xdg"
}
trap cleanup EXIT INT TERM

if [[ "${SCOREPEEK_PREBUILT_SKINS:-0}" != 1 ]]; then
  "$root/scripts/build-skins.sh"
fi
export SCOREPEEK_PREBUILT_SKINS=1

export XDG_DATA_HOME="$xdg/data"
export XDG_CONFIG_HOME="$xdg/config"
export XDG_CACHE_HOME="$xdg/cache"
if [[ "${SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR:-0}" != 1 ]]; then
  export XDG_RUNTIME_DIR="$xdg/runtime"
  mkdir -p "$XDG_RUNTIME_DIR"
  chmod 700 "$XDG_RUNTIME_DIR"
fi

cargo run --locked -p scorepeek-overlay --example install_skins -- "$root"/target/skins/*.zip

"$@"
