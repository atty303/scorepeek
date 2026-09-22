#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo 'usage: visual-wayland.bash NEW_CONFIG.toml [SECONDS]' >&2
  exit 2
fi
config_path=$1
seconds=${2:-30}
if [[ ! "$seconds" =~ ^[1-9][0-9]*$ || -e "$config_path" ]]; then
  echo 'SECONDS must be positive and CONFIG.toml must not already exist' >&2
  exit 2
fi
mkdir -p "$(dirname "$config_path")"
config_path=$(realpath -m -- "$config_path")
root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-visual-wayland.XXXXXX")
fifo="$root/stop"
mkfifo "$fifo"
exec 3<>"$fifo"
cleanup() {
  status=$?
  exec 3>&- || true
  if [[ -n "${server_pid:-}" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" || true
  fi
  if [[ "$status" -ne 0 ]]; then
    for log in "$root/server.log" "$root/role.stdout" "$root/role.stderr"; do
      if [[ -f "$log" ]]; then sed -n '1,240p' "$log" >&2; fi
    done
  fi
  find "$root" -depth -delete
  return "$status"
}
trap cleanup EXIT
SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR=1 scripts/with-isolated-skins.sh node scripts/overlay-fixture-host.js "$root" 127.0.0.1:0 - "$config_path" wayland <"$fifo" 3>&- >"$root/server.log" 2>&1 &
server_pid=$!
for _ in {1..200}; do
  if [[ -f "$root/role.stdout" ]] && rg -q '"operation":"child_ready"' "$root/role.stdout"; then break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then exit 1; fi
  sleep 0.1
done
rg -q '"operation":"child_ready"' "$root/role.stdout"
sleep "$seconds"
exec 3>&-
wait "$server_pid"
server_pid=
