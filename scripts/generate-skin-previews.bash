#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
run_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skin-previews.XXXXXX")
stop_fifo="$run_root/stop"
server_log="$run_root/server.log"
config_path="$run_root/overlay.toml"
source_config="$run_root/preview-source.toml"
staging="$run_root/output"
port=$(node -e 'const net=require("node:net");const server=net.createServer();server.listen(0,"127.0.0.1",()=>{process.stdout.write(String(server.address().port));server.close();});')
address="127.0.0.1:$port"
mkdir -p "$staging"
mkfifo "$stop_fifo"
exec 3<>"$stop_fifo"

cleanup() {
  status=$?
  exec 3>&- || true
  if [[ -n "${server_pid:-}" ]]; then
    wait "$server_pid" || true
  fi
  if [[ "$status" -ne 0 && -f "$server_log" ]]; then
    sed -n '1,240p' "$server_log" >&2
  fi
  find "$run_root" -depth -delete
  return "$status"
}
trap cleanup EXIT

node "$root/scripts/write-skin-preview-config.js" "$root/skins/preview-scene.json" "$source_config"
"$root/scripts/with-isolated-skins.sh" \
  "$root/target/debug/examples/visual_obs" "$config_path" "$address" "$source_config" \
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
playwright test tests/skin-preview-browser.spec.js --workers=1 --reporter=line --output="$run_root/playwright"

destination=${1:-}
if [[ -n "$destination" ]]; then
  mkdir -p "$destination"
  cp -a "$staging/." "$destination/"
else
  while IFS= read -r slug; do
    install -m 0644 "$staging/$slug/preview.png" "$root/skins/$slug/preview.png"
    install -m 0644 "$staging/$slug/preview.webm" "$root/skins/$slug/preview.webm"
  done < <(node -e 'const scene=require(process.argv[1]);for(const skin of scene.skins)console.log(skin.slug)' "$root/skins/preview-scene.json")
  "$root/scripts/build-skins.sh"
  mkdir -p "$root/target/skin-previews"
  install -m 0644 "$staging/manifest.json" "$root/target/skin-previews/manifest.json"
fi
