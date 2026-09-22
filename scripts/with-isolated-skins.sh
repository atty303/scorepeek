#!/usr/bin/env bash
set -euo pipefail

if [[ "${SCOREPEEK_ISOLATED_SKINS:-0}" == 1 ]]; then
  exec "$@"
fi

root=$(cd "$(dirname "$0")/.." && pwd)
node_binary=$(node -p 'process.execPath')
xdg=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skins.XXXXXX")
export SCOREPEEK_ISOLATED_SKINS=1
export MISE_DATA_DIR="${MISE_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mise}"
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/ms-playwright}"
cleanup() {
  if [[ -n "${active_child:-}" ]] && kill -0 "$active_child" 2>/dev/null; then
    kill "$active_child" 2>/dev/null || true
    for _ in {1..50}; do
      if ! kill -0 "$active_child" 2>/dev/null; then break; fi
      sleep 0.1
    done
    if kill -0 "$active_child" 2>/dev/null; then kill -KILL "$active_child" 2>/dev/null || true; fi
    wait "$active_child" 2>/dev/null || true
  fi
  rm -rf "$xdg"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_child() {
  "$@" <&0 &
  active_child=$!
  wait "$active_child"
  active_child=
}

if [[ "${SCOREPEEK_PREBUILT_SKINS:-0}" != 1 ]]; then
  run_child "$root/scripts/build-skins.sh"
fi
export SCOREPEEK_PREBUILT_SKINS=1

export HOME="$xdg/home"
export XDG_DATA_HOME="$xdg/data"
export XDG_CONFIG_HOME="$xdg/config"
export XDG_STATE_HOME="$xdg/state"
export XDG_CACHE_HOME="$xdg/cache"
mkdir -p "$HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
if [[ "${SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR:-0}" != 1 ]]; then
  export XDG_RUNTIME_DIR="$xdg/runtime"
  mkdir -p "$XDG_RUNTIME_DIR"
  chmod 700 "$XDG_RUNTIME_DIR"
fi

for package in "$root"/target/skins/*.zip; do
  "${SCOREPEEK_BINARY:-$root/target/debug/scorepeek}" skin install "$package" >/dev/null
done

if [[ "${1:-}" == node ]]; then
  shift
  set -- "$node_binary" "$@"
fi
run_child "$@"
