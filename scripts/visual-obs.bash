#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 ]]; then
  echo 'usage: visual-obs.bash NEW_CONFIG.toml 127.0.0.1:PORT' >&2
  exit 2
fi
config_path=$1
address=$2
if [[ -e "$config_path" ]]; then
  echo 'CONFIG.toml must not already exist' >&2
  exit 2
fi
mkdir -p "$(dirname "$config_path")"
config_path=$(realpath -m -- "$config_path")
root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-visual-obs.XXXXXX")
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
scripts/with-isolated-skins.sh node scripts/overlay-fixture-host.js "$root" "$address" - "$config_path" <"$fifo" 3>&- >"$root/server.log" 2>&1 &
server_pid=$!
for _ in {1..200}; do
  if curl --fail --silent --output /dev/null "http://$address/overlay"; then break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then exit 1; fi
  sleep 0.1
done
curl --fail --silent --output /dev/null "http://$address/overlay"
printf 'Open http://%s/overlay and press Enter to stop.\n' "$address"
read -r _ || true
