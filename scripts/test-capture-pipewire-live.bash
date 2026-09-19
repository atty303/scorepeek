#!/usr/bin/env bash
set -euo pipefail

scorepeek_bin=${1:-target/debug/scorepeek}
node_name=${SCOREPEEK_PIPEWIRE_TEST_NODE:-scorepeek-live-test}
source_data_home=${XDG_DATA_HOME:-${HOME}/.local/share}
test_root=$(mktemp -d)
scorepeek_pid=
producer_pid=

cleanup() {
  local status=$?
  if [[ -n ${producer_pid} ]]; then
    kill -INT "${producer_pid}" 2>/dev/null || true
    wait "${producer_pid}" 2>/dev/null || true
  fi
  if [[ -n ${scorepeek_pid} ]]; then
    kill -INT "${scorepeek_pid}" 2>/dev/null || true
    wait "${scorepeek_pid}" 2>/dev/null || true
  fi
  if (( status != 0 )); then
    sed -n '1,240p' "${test_root}/scorepeek.stderr" >&2 || true
    sed -n '1,120p' "${test_root}/scorepeek.stdout" >&2 || true
    find "${test_root}" -name diagnostics.ndjson -type f -exec tail -80 {} \; >&2 || true
  fi
  rm -rf -- "${test_root}"
  return "${status}"
}
trap cleanup EXIT

command -v gst-launch-1.0 >/dev/null
command -v zig >/dev/null
test -x "${scorepeek_bin}"
mkdir -p "${test_root}/data/scorepeek"
ln -s "${source_data_home}/scorepeek/catalog" "${test_root}/data/scorepeek/catalog"
read -r -a pipewire_flags <<<"$(scripts/pkg-config-scorepeek.bash --cflags libpipewire-0.3)"
ZIG_LOCAL_CACHE_DIR="${test_root}/zig-cache" ZIG_GLOBAL_CACHE_DIR="${test_root}/zig-global-cache" zig cc scripts/pipewire-contract-source.c -o "${test_root}/pipewire-contract-source" "${pipewire_flags[@]}" /usr/lib64/libpipewire-0.3.so.0

run_producer() {
  local width=$1
  local height=$2
  local pattern=$3
  gst-launch-1.0 -q videotestsrc is-live=true pattern="${pattern}" ! "video/x-raw,format=BGRx,width=${width},height=${height},framerate=30/1" ! pipewiresink mode=provide client-name="${node_name}" stream-properties="props,node.name=${node_name},media.class=Video/Source" &
  producer_pid=$!
}

XDG_DATA_HOME="${test_root}/data" XDG_STATE_HOME="${test_root}/state" "${scorepeek_bin}" run --capture pipewire --node-name "${node_name}" --crop-left 8 --crop-top 4 --crop-right 8 --crop-bottom 4 --no-scores >"${test_root}/scorepeek.stdout" 2>"${test_root}/scorepeek.stderr" &
scorepeek_pid=$!

run_producer 1280 720 smpte
sleep 3
kill -INT "${producer_pid}"
wait "${producer_pid}" || true
producer_pid=
sleep 1

run_producer 1024 768 ball
sleep 3
kill -INT "${producer_pid}"
wait "${producer_pid}" || true
producer_pid=
sleep 1

"${test_root}/pipewire-contract-source" "${node_name}" &
producer_pid=$!
wait "${producer_pid}"
producer_pid=
sleep 1

if ! kill -0 "${scorepeek_pid}" 2>/dev/null; then
  wait "${scorepeek_pid}" || true
  echo "scorepeek exited before the live verification completed" >&2
  exit 1
fi
kill -INT "${scorepeek_pid}"
wait "${scorepeek_pid}"
scorepeek_pid=

diagnostics=$(find "${test_root}/state/scorepeek/diagnostics" -name diagnostics.ndjson -type f -print -quit)
test -n "${diagnostics}"
grep -q '"capture_generation":1' "${diagnostics}"
grep -q '"capture_generation":2' "${diagnostics}"
grep -q '"capture_generation":3' "${diagnostics}"
grep -q '"capture_generation":4' "${diagnostics}"
grep -q '\\"width\\":1280' "${diagnostics}"
grep -q '\\"width\\":1024' "${diagnostics}"
grep -q '\\"width\\":640' "${diagnostics}"
grep -q '\\"width\\":800' "${diagnostics}"
grep -q 'source_contract_changed' "${diagnostics}"
grep -Eq '"operation":"frame_normalization".*"status":"success"' "${diagnostics}"
grep -q 'performance_summary' "${diagnostics}"
if grep -q '"outcome":"error"' "${diagnostics}"; then
  echo "capture generation ended with an error" >&2
  exit 1
fi

echo "generic PipeWire source disappearance, reappearance, same-node contract change, crop admission, and generation switch passed"
