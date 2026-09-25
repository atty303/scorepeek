#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
run_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skin-previews.XXXXXX")
stop_fifo="$run_root/stop"
server_log="$run_root/server.log"
staging="$run_root/output"
port=$(deno eval 'const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 }); console.log(listener.addr.port); listener.close();')
address="127.0.0.1:$port"
mkdir -p "$staging"
mkfifo "$stop_fifo"
exec 3<>"$stop_fifo"

cleanup() {
  status=$?
  exec 3>&- || true
  if [[ -n "${server_pid:-}" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" || true
  fi
  if [[ "$status" -ne 0 ]]; then
    for log in "$server_log" "$run_root/role.stdout" "$run_root/role.stderr"; do
      if [[ -f "$log" ]]; then sed -n '1,240p' "$log" >&2; fi
    done
  fi
  find "$run_root" -depth -delete
  return "$status"
}
trap cleanup EXIT

"$root/scripts/with-isolated-skins.sh" \
  deno run -A "$root/scripts/overlay-fixture-host.deno.js" "$run_root" "$address" "$root/skins/preview-scene.json" \
  <"$stop_fifo" 3>&- >"$server_log" 2>&1 &
server_pid=$!

for _ in {1..200}; do
  if curl --fail --silent --output /dev/null "http://$address/overlay"; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    sed -n '1,240p' "$server_log" >&2
    exit 1
  fi
  sleep 0.1
done
curl --fail --silent --output /dev/null "http://$address/overlay"

SCOREPEEK_SKIN_PREVIEW_URL="http://$address" \
SCOREPEEK_SKIN_PREVIEW_OUTPUT="$staging" \
deno test -A scripts/generate-skin-previews.browser.test.js

destination=${1:-}
if [[ -n "$destination" ]]; then
  mkdir -p "$destination"
  cp -a "$staging/." "$destination/"
else
  while IFS= read -r slug; do
    install -m 0644 "$staging/$slug/preview.png" "$root/skins/$slug/preview.png"
    install -m 0644 "$staging/$slug/preview.webm" "$root/skins/$slug/preview.webm"
  done < <(deno eval 'const scene = JSON.parse(await Deno.readTextFile(Deno.args[0])); for (const skin of scene.skins) console.log(skin.slug)' "$root/skins/preview-scene.json")
  "$root/scripts/build-skins.sh"
  mkdir -p "$root/target/skin-previews"
  install -m 0644 "$staging/manifest.json" "$root/target/skin-previews/manifest.json"
fi
