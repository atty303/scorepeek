#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
  --report-outputs)
    sleep 2
    scrollmsg -t get_outputs >"${2:?missing scenario root}/outputs.json"
    exit
    ;;
  --exercise-input)
    root=${2:?missing scenario root}
    sleep 5
    scrollmsg seat - cursor set 260 90 >/dev/null
    scrollmsg seat - cursor press button1 >/dev/null
    scrollmsg seat - cursor move 50 50 >/dev/null
    scrollmsg seat - cursor release button1 >/dev/null
    printf '%s\n' 'pointer drag injected through compositor IPC' >"$root/input.txt"
    exit
    ;;
  --exit-after)
    sleep 50
    scrollmsg exit >/dev/null
    exit
    ;;
  --run-scenario)
    root=${2:?missing scenario root}
    repo=${3:?missing repository root}
    cd "$repo"
    mkfifo "$root/stop"
    exec 3<>"$root/stop"
    SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR=1 scripts/with-isolated-skins.sh deno run -A scripts/overlay-fixture-host.deno.js "$root" 127.0.0.1:0 - "$root/overlay.toml" wayland <"$root/stop" 3>&- >"$root/host.log" 2>&1 &
    host_pid=$!
    scenario_cleanup() {
      if [[ -n "${host_pid:-}" ]]; then
        kill "$host_pid" 2>/dev/null || true
        wait "$host_pid" 2>/dev/null || true
      fi
    }
    trap scenario_cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    for _ in {1..200}; do
      if [[ -f "$root/role.stdout" ]] && rg -q '"operation":"child_ready"' "$root/role.stdout"; then break; fi
      if ! kill -0 "$host_pid" 2>/dev/null; then break; fi
      sleep 0.1
    done
    if ! rg -q '"operation":"child_ready"' "$root/role.stdout"; then
      cat "$root/host.log" "$root/role.stderr" >&2
      kill "$host_pid" 2>/dev/null || true
      wait "$host_pid" || true
      exit 1
    fi
    sleep 20
    exec 3>&-
    wait "$host_pid"
    host_pid=
    scrollmsg exit >/dev/null
    exit
    ;;
  '') ;;
  *) echo 'unknown nested Wayland helper mode' >&2; exit 2 ;;
esac

repo=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-nested-wayland.XXXXXX")
mkdir "$root/runtime"
chmod 700 "$root/runtime"
cleanup() {
  status=$?
  if [[ "$status" -ne 0 ]]; then
    for log in "$root/scroll.log" "$root/host.log" "$root/role.stderr" "$root/role.stdout"; do
      if [[ -f "$log" ]]; then tail -n 120 "$log" >&2; fi
    done
  fi
  find "$root" -depth -delete
  return "$status"
}
trap cleanup EXIT

cat >"$root/scroll.conf" <<EOF
output HEADLESS-1 mode 1920x1080@120Hz
output HEADLESS-2 mode 1280x720@60Hz
animations { enabled no }
swaybg_command -
exec "$repo/scripts/test-overlay-wayland-nested.bash" --run-scenario "$root" "$repo"
exec "$repo/scripts/test-overlay-wayland-nested.bash" --report-outputs "$root"
exec "$repo/scripts/test-overlay-wayland-nested.bash" --exercise-input "$root"
exec "$repo/scripts/test-overlay-wayland-nested.bash" --exit-after
EOF

timeout 70s env XDG_RUNTIME_DIR="$root/runtime" WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 scroll -c "$root/scroll.conf" >"$root/scroll.log" 2>&1
jq -e '[.[] | .name] | index("HEADLESS-1") != null and index("HEADLESS-2") != null' "$root/outputs.json" >/dev/null
test -s "$root/input.txt"
jq -Rc 'fromjson? | select(.operation == "native_summary" and .data.status == "complete") | .data.output_name' "$root/role.stdout" | jq -es 'index("HEADLESS-1") != null and index("HEADLESS-2") != null' >/dev/null
if jq -Rc 'fromjson? | select(.operation == "native_canvas_failed")' "$root/role.stdout" | rg -q .; then
  echo 'production Wayland canvas failed' >&2
  exit 1
fi
printf 'nested production Wayland role painted both outputs and received compositor pointer input\n'
