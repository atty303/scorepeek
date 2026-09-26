#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: verify-skin-review-browser.bash NATIVE_SCENE_DIR SLUG NEW_OUTPUT_DIR" >&2
  exit 2
fi
root=$(cd "$(dirname "$0")/../../../.." && pwd)
skill="$root/.agents/skills/create-overlay-skin"
native_scenes=$1
slug=$2
output=$3
mkdir "$output"
run_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-skin-review-browser.XXXXXX")
server_pid=
cleanup() {
  status=$?
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf -- "$run_root"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

deno run --allow-read --allow-write "$skill/scripts/prepare-browser-review-scenes.ts" \
  "$native_scenes" "$run_root/scenes" "$slug"
if [[ "${SCOREPEEK_PREBUILT_SKINS:-0}" != 1 ]]; then
  bash "$root/scripts/build-skins.sh"
fi
case_ids=(01 02 03 04 05 06 07 08 rank-a rank-aa rank-aaa background-off background-static background-animated)
if [[ ! -f "$native_scenes/background-animated.json" ]]; then
  case_ids=(01 02 03 04 05 06 07 08 rank-a rank-aa rank-aaa background-off background-static)
fi
if [[ -n "${SCOREPEEK_REVIEW_CASE_ID:-}" ]]; then
  if [[ ! "$SCOREPEEK_REVIEW_CASE_ID" =~ ^(0[1-8]|rank-(a|aa|aaa)|background-(off|static|animated))$ ]]; then
    echo "SCOREPEEK_REVIEW_CASE_ID must be 01..08, rank-a/rank-aa/rank-aaa, or background-off/background-static/background-animated" >&2
    exit 2
  fi
  if [[ ! -f "$native_scenes/$SCOREPEEK_REVIEW_CASE_ID.json" ]]; then
    echo "review case not generated: $SCOREPEEK_REVIEW_CASE_ID" >&2
    exit 2
  fi
  case_ids=("$SCOREPEEK_REVIEW_CASE_ID")
  if [[ "$SCOREPEEK_REVIEW_CASE_ID" != 01 ]]; then
    case_ids=(01 "$SCOREPEEK_REVIEW_CASE_ID")
  fi
fi
for case_id in "${case_ids[@]}"; do
  case_root="$run_root/$case_id"
  mkdir "$case_root"
  fifo="$case_root/stop"
  mkfifo "$fifo"
  exec 3<>"$fifo"
  port=$(deno eval 'const socket = Deno.listen({hostname:"127.0.0.1",port:0}); console.log(socket.addr.port); socket.close();')
  address="127.0.0.1:$port"
  SCOREPEEK_PREBUILT_SKINS=1 "$root/scripts/with-isolated-skins.sh" \
    deno run -A "$root/scripts/overlay-fixture-host.deno.js" "$case_root" "$address" "$run_root/scenes/$case_id.json" \
    <"$fifo" 3>&- >"$case_root/server.log" 2>&1 &
  server_pid=$!
  ready=false
  for _ in {1..200}; do
    if curl --fail --silent --output /dev/null "http://$address/overlay"; then ready=true; break; fi
    if ! kill -0 "$server_pid" 2>/dev/null; then break; fi
    sleep 0.1
  done
  if [[ "$ready" != true ]]; then cat "$case_root/server.log" >&2; exit 1; fi
  SCOREPEEK_SKIN_REVIEW_SCENE="$run_root/scenes/$case_id.json" \
  SCOREPEEK_SKIN_REVIEW_URL="http://$address" \
  SCOREPEEK_SKIN_REVIEW_SCREENSHOT="$output/$case_id.png" \
    deno test -A "$skill/scripts/verify-skin-review.browser.test.js"
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" || true
  server_pid=
  exec 3>&-
  sha256sum "$output/$case_id.png" >> "$output/SHA256SUMS"
done
if [[ -z "${SCOREPEEK_REVIEW_CASE_ID:-}" ]]; then
  deno run --allow-read --allow-write --allow-run=magick,ffmpeg,ffprobe \
    "$skill/scripts/compose-browser-review-video.ts" \
    "$native_scenes" "$output" "$output/review.webm"
fi
