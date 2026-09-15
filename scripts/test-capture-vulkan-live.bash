#!/usr/bin/env bash
set -euo pipefail

scorepeek_bin=${1:-target/debug/scorepeek}
numeric_model_bundle=${SCOREPEEK_NUMERIC_MODEL_BUNDLE:-}
source_data_home=${XDG_DATA_HOME:-${HOME}/.local/share}
test_root=$(mktemp -d)
manifest_dir=$(realpath target/vulkan-capture/share/vulkan/explicit_layer.d)
scorepeek_pid=
game_pid=

cleanup() {
  local status=$?
  if [[ -n ${game_pid} ]]; then
    kill -INT "${game_pid}" 2>/dev/null || true
    wait "${game_pid}" 2>/dev/null || true
  fi
  if [[ -n ${scorepeek_pid} ]]; then
    kill -INT "${scorepeek_pid}" 2>/dev/null || true
    wait "${scorepeek_pid}" 2>/dev/null || true
  fi
  if (( status != 0 )); then
    tail -120 "${test_root}/gamescope.log" >&2 || true
    sed -n '1,160p' "${test_root}/first.stderr" >&2 || true
    sed -n '1,160p' "${test_root}/second.stderr" >&2 || true
    find "${test_root}" -name diagnostics.ndjson -type f -exec sed -n '1,240p' {} \; >&2 || true
  fi
  rm -rf -- "${test_root}"
  return "${status}"
}
trap cleanup EXIT

for command in gamescope vkcube obs-vkcapture; do
  command -v "${command}" >/dev/null
done
test -x "${scorepeek_bin}"
test -d "${numeric_model_bundle}" || {
  echo "SCOREPEEK_NUMERIC_MODEL_BUNDLE must name the registered private numeric-model bundle" >&2
  exit 1
}
mkdir -p "${test_root}/data/scorepeek"
ln -s "${source_data_home}/scorepeek/catalog" "${test_root}/data/scorepeek/catalog"
XDG_DATA_HOME="${test_root}/data" "${scorepeek_bin}" numeric-model install --bundle "${numeric_model_bundle}" >/dev/null

start_scorepeek() {
  local state_root=$1
  XDG_DATA_HOME="${test_root}/data" XDG_STATE_HOME="${state_root}" "${scorepeek_bin}" run --capture vulkan-layer --no-scores >"${state_root}.stdout" 2>"${state_root}.stderr" &
  scorepeek_pid=$!
  for _ in $(seq 1 100); do
    if [[ -S ${XDG_RUNTIME_DIR}/scorepeek/vulkan-capture.sock ]]; then
      return
    fi
    if ! kill -0 "${scorepeek_pid}" 2>/dev/null; then
      wait "${scorepeek_pid}"
    fi
    sleep 0.05
  done
  echo "Scorepeek Vulkan socket did not appear" >&2
  exit 1
}

wait_for_captured_frame() {
  local state_root=$1
  for _ in $(seq 1 400); do
    if find "${state_root}/scorepeek/diagnostics" -name diagnostics.ndjson -type f -exec grep -Eq '"operation":"frame_normalization".*"status":"success"' {} + 2>/dev/null; then
      return
    fi
    if ! kill -0 "${scorepeek_pid}" 2>/dev/null; then
      wait "${scorepeek_pid}"
    fi
    sleep 0.05
  done
  echo "Vulkan capture did not produce a successfully normalized frame" >&2
  exit 1
}

first_state="${test_root}/first"
second_state="${test_root}/second"
start_scorepeek "${first_state}"

gamescope -W 1280 -H 720 -r 60 -- env VK_LOADER_DEBUG=layer VK_LAYER_PATH="${manifest_dir}" VK_INSTANCE_LAYERS=VK_LAYER_SCOREPEEK_capture OBS_VKCAPTURE=1 vkcube >"${test_root}/gamescope.log" 2>&1 &
game_pid=$!
wait_for_captured_frame "${first_state}"

kill -INT "${scorepeek_pid}"
wait "${scorepeek_pid}"
scorepeek_pid=

start_scorepeek "${second_state}"
wait_for_captured_frame "${second_state}"

kill -INT "${game_pid}"
wait "${game_pid}" || true
game_pid=
sleep 1
kill -INT "${scorepeek_pid}"
wait "${scorepeek_pid}"
scorepeek_pid=

grep -q 'VK_LAYER_SCOREPEEK_capture' "${test_root}/gamescope.log"
grep -qi 'obs_vkcapture' "${test_root}/gamescope.log"
for state_root in "${first_state}" "${second_state}"; do
  diagnostics=$(find "${state_root}/scorepeek/diagnostics" -name diagnostics.ndjson -type f -print -quit)
  test -n "${diagnostics}"
  grep -q 'capture_generation_identity' "${diagnostics}"
  grep -q '\\"backend\\":\\"vulkan_layer\\"' "${diagnostics}"
  grep -q '\\"vulkan_source\\"' "${diagnostics}"
  grep -q 'performance_summary' "${diagnostics}"
  grep -Eq '"detail":\{"count":[1-9][0-9]*,[^}]*"kind":"performance_summary"' "${diagnostics}"
  if grep -q '"outcome":"error"' "${diagnostics}"; then
    echo "Vulkan capture generation ended with an error" >&2
    exit 1
  fi
done

echo "Vulkan consumer, Gamescope/vkcube/obs-vkcapture coexistence, and Scorepeek reconnection passed"
