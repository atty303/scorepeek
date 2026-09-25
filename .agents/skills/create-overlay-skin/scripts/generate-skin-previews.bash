#!/usr/bin/env bash
set -euo pipefail

skill=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$skill/../../.." && pwd)
run_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skin-previews.XXXXXX")
stop_fifo="$run_root/stop"
server_log="$run_root/server.log"
staging="$run_root/output"
port=$(deno eval 'const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 }); console.log(listener.addr.port); listener.close();')
address="127.0.0.1:$port"
scene_path=${SCOREPEEK_SKIN_PREVIEW_SCENE:-$skill/preview-scene.json}
skins_directory=${SCOREPEEK_SKINS_DIRECTORY:-$root/skins}
deno run --allow-read --allow-write "$skill/scripts/prepare-preview-scene.ts" \
  "$scene_path" "$run_root/scene.json" "$skins_directory" "${SCOREPEEK_SKIN_PREVIEW_SLUG:-}"
scene_path=$run_root/scene.json
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
  deno run -A "$root/scripts/overlay-fixture-host.deno.js" "$run_root" "$address" "$scene_path" \
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
SCOREPEEK_SKIN_PREVIEW_SCENE="$scene_path" \
SCOREPEEK_SKINS_DIRECTORY="$skins_directory" \
deno test -A "$skill/scripts/generate-skin-previews.browser.test.js"

destination=${1:-}
if [[ -n "$destination" ]]; then
  mkdir -p "$destination"
  cp -a "$staging/." "$destination/"
else
  while IFS= read -r slug; do
    install -m 0644 "$staging/$slug/preview.png" "$skins_directory/$slug/preview.png"
    install -m 0644 "$staging/$slug/preview.webm" "$skins_directory/$slug/preview.webm"
  done < <(deno eval 'const scene = JSON.parse(await Deno.readTextFile(Deno.args[0])); for (const skin of scene.skins) console.log(skin.slug)' "$scene_path")
  "$root/scripts/build-skins.sh"
  mkdir -p "$root/target/skin-previews"
  install -m 0644 "$staging/manifest.json" "$root/target/skin-previews/manifest.json"
fi
