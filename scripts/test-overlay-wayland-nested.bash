#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != --run ]]; then
  exec timeout --signal=TERM --kill-after=15s 300s "$0" --run
fi

image=ghcr.io/atty303/arch-scroll@sha256:b5613820c8333a8dedc9e6329423db3bcca957ed0baa74365a7239f9f02b36a8
repo=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-nested-wayland.XXXXXX")
container=scorepeek-nested-wayland-${root##*.}
runtime=$root/runtime
container_runtime=/tmp/scorepeek-runtime
icd=
host_pid=

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [[ -n "$host_pid" ]]; then
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
  fi
  container_id=
  if [[ -s "$root/container.id" ]]; then
    container_id=$(<"$root/container.id")
  elif [[ $(podman inspect "$container" --format '{{ index .Config.Labels "org.scorepeek.nested-root" }}' 2>/dev/null || true) == "$root" ]]; then
    container_id=$container
  fi
  if [[ -n "$container_id" ]]; then
    podman logs "$container_id" >"$root/scroll.log" 2>&1 || true
    podman stop --time 3 "$container_id" >/dev/null 2>&1 || true
    podman rm --force "$container_id" >/dev/null 2>&1 || true
  fi
  if ((status != 0)); then
    printf 'nested Wayland test failed (exit %s)\n' "$status" >&2
    for log in "$root/container-start.stderr" "$root/container-start.stdout" "$root/scroll.log" "$root/host.log" "$root/input-reply.json" "$root/role.stderr" "$root/role.stdout"; do
      if [[ -f "$log" ]]; then
        printf '== %s ==\n' "${log##*/}" >&2
        tail -n 120 "$log" | cut -c 1-2000 >&2
      fi
    done
  fi
  rm -rf -- "$root"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

wait_for() {
  local label=$1
  shift
  local deadline=$((SECONDS + 30))
  until "$@"; do
    if ((SECONDS >= deadline)); then
      printf 'timed out waiting for %s\n' "$label" >&2
      return 1
    fi
    sleep 0.1
  done
}

scroll_ready() {
  [[ -S "$runtime/wayland-1" ]] || return 1
  ipc_socket=$(find "$runtime" -maxdepth 1 -name 'scroll-ipc.*.sock' -print -quit)
  [[ -n "$ipc_socket" ]] || return 1
  scrollmsg -t get_outputs >"$root/outputs.json" 2>/dev/null &&
    jq -e '[.[] | .name] | index("HEADLESS-1") != null and index("HEADLESS-2") != null' "$root/outputs.json" >/dev/null
}

scrollmsg() {
  podman exec -e "SWAYSOCK=$container_runtime/${ipc_socket##*/}" "$container" scrollmsg "$@"
}

child_ready() {
  [[ -f "$root/role.stdout" ]] &&
    jq -e -s 'any(.[]; .operation == "child_ready")' "$root/role.stdout" >/dev/null 2>&1
}

painted_both_outputs() {
  [[ -f "$root/role.stdout" ]] &&
    jq -e -s '[.[] | select(.operation == "native_editor_painted") | .data.output] | index("HEADLESS-1") != null and index("HEADLESS-2") != null' "$root/role.stdout" >/dev/null 2>&1
}

painted_after_input() {
  (( $(paint_count) > $1 ))
}

paint_count() {
  jq -Rc 'fromjson? | select(.operation == "native_editor_painted")' "$root/role.stdout" | wc -l
}

inject_pointer() {
  scrollmsg "$@" | jq -e 'all(.[]; .success == true)' >"$root/input-reply.json"
}

software_vulkan_on_both_outputs() {
  jq -e -s '[.[] | select(.operation == "surface_configured" and .data.gpu_backend == "Vulkan" and (.data.gpu_adapter | test("llvmpipe|lavapipe"; "i"))) | .data.output_name] | index("HEADLESS-1") != null and index("HEADLESS-2") != null' "$root/role.stdout" >/dev/null
}

mkdir "$runtime"
chmod 700 "$runtime"
for candidate in /usr/share/vulkan/icd.d/lvp_icd.json /usr/share/vulkan/icd.d/lvp_icd.x86_64.json; do
  if [[ -f "$candidate" ]]; then
    icd=$candidate
    break
  fi
done
if [[ -z "$icd" ]]; then
  printf 'Mesa software Vulkan ICD missing (lvp_icd.json or lvp_icd.x86_64.json)\n' >&2
  exit 1
fi
printf 'Mesa software Vulkan ICD: %s\n' "$icd"
if ! command -v podman >/dev/null || ! command -v jq >/dev/null; then
  printf 'nested Wayland test requires podman and jq\n' >&2
  exit 1
fi

cat >"$root/scroll.conf" <<'EOF'
output HEADLESS-1 mode 1920x1080@120Hz
output HEADLESS-2 mode 1280x720@60Hz
animations {
    enabled no
}
swaybg_command -
EOF

podman run --detach --pull=missing --name "$container" --cidfile "$root/container.id" \
  --label "org.scorepeek.nested-root=$root" --network=none \
  --userns=keep-id --security-opt label=disable \
  --volume "$runtime:$container_runtime:rw" \
  --volume "$root/scroll.conf:/tmp/scorepeek-scroll.conf:ro" \
  --env "XDG_RUNTIME_DIR=$container_runtime" --env WAYLAND_DISPLAY=wayland-1 \
  --env WLR_BACKENDS=headless --env WLR_HEADLESS_OUTPUTS=2 --env WLR_RENDERER=pixman \
  --entrypoint scroll "$image" -c /tmp/scorepeek-scroll.conf >"$root/container-start.stdout" 2>"$root/container-start.stderr"
ipc_socket=
wait_for 'Scroll Wayland socket, IPC and two outputs' scroll_ready

mkfifo "$root/stop"
exec 3<>"$root/stop"
XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY=wayland-1 \
  VK_DRIVER_FILES="$icd" VK_ICD_FILENAMES="$icd" \
  SCOREPEEK_PREBUILT_SKINS=1 SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR=1 \
  "$repo/scripts/with-isolated-skins.sh" deno run -A "$repo/scripts/overlay-fixture-host.deno.js" \
  "$root" 127.0.0.1:0 - "$root/overlay.toml" wayland \
  <"$root/stop" 3>&- >"$root/host.log" 2>&1 &
host_pid=$!
wait_for 'Scorepeek child readiness' child_ready
wait_for 'initial paint on both outputs' painted_both_outputs
software_vulkan_on_both_outputs

before_paints=$(paint_count)
inject_pointer seat - cursor set 260 90
inject_pointer seat - cursor press button1
inject_pointer seat - cursor move 50 50
inject_pointer seat - cursor release button1
wait_for 'paint after Scroll pointer drag' painted_after_input "$before_paints"

exec 3>&-
wait "$host_pid"
host_pid=
jq -e -s '[.[] | select(.operation == "native_summary" and .data.status == "complete") | .data.output_name] | index("HEADLESS-1") != null and index("HEADLESS-2") != null' "$root/role.stdout" >/dev/null
if jq -e -s 'any(.[]; .operation == "native_canvas_failed")' "$root/role.stdout" >/dev/null; then
  echo 'production Wayland canvas failed' >&2
  exit 1
fi
printf 'nested production Wayland role used software Vulkan, painted both outputs and repainted after compositor pointer drag\n'
