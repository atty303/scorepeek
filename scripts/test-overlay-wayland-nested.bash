#!/usr/bin/env bash
set -euo pipefail
trap 'echo "nested assertion failed at line $LINENO" >&2' ERR

if [[ "${1:-}" == "--exit-compositor-after" ]]; then
  sleep "${2:?missing compositor exit delay}"
  scrollmsg exit
  exit 0
fi

if [[ "${1:-}" == "--exercise-input" ]]; then
  sleep 5
  "${2:?missing virtual pointer executable}" "${3:?missing feed trigger socket}" "${4:?missing diagnostic log}"
  exit 0
fi

if [[ "${1:-}" == "--report-outputs" ]]; then
  sleep 1
  scrollmsg -t get_outputs | jq -c '{operation:"nested_output_layout",outputs:[.[]|{name,rect}]}'
  exit 0
fi

if [[ "${1:-}" == "--run-editor-scenario" ]]; then
  repo_root="${2:?missing repository root}"
  overlay_config="${3:?missing overlay config}"
  cd "$repo_root"
  SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR=1 scripts/with-isolated-skins.sh target/release/examples/visual_wayland "$overlay_config" 30 --integration-fixture
  scrollmsg exit >/dev/null 2>&1 || true
  exit 0
fi

test_root="$(mktemp -d /tmp/scorepeek-overlay-wayland-nested.XXXXXX)"
config="$test_root/scroll.conf"
overlay_config="$test_root/overlay.toml"
log="$test_root/scroll.log"
repo_root="$(pwd)"

cleanup() {
  status=$?
  trap - ERR
  if [[ "$status" -ne 0 && -f "$log" ]]; then
    rg 'nested_output_layout' "$log" || true
    json_log | jq -c 'select(.operation == "native_summary") | {operation,data:{run_id:.data.run_id,canvas_id:.data.canvas_id,output_name:.data.output_name,status:.data.status,effective_paint_hz:.data.effective_paint_hz,paint_count:.data.paint_count,skin_runtime_create_count:.data.skin_runtime_create_count}}' || true
    json_log | jq -c 'select((.operation == "native_surface_transition" or .operation == "native_surface_visibility" or .operation == "native_canvas_failed") and .data.canvas_id == "wayland-selection") | {operation,sequence,timestamp_unix_us,data}' || true
  fi
  find "$test_root" -depth -delete
  return "$status"
}
trap cleanup EXIT

json_log() {
  jq -Rc 'fromjson?' "$log"
}

cat >"$config" <<EOF
output HEADLESS-1 mode 1920x1080@120Hz
output HEADLESS-2 mode 1280x720@60Hz
animations { enabled no }
swaybg_command -
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --run-editor-scenario "$repo_root" "$overlay_config"
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --report-outputs
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --exercise-input "$repo_root/target/release/examples/virtual_pointer" "$test_root/feed-trigger.sock" "$log"
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --exit-compositor-after 60
EOF

WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 scroll -c "$config" >"$log" 2>&1

summary_count="$(json_log | jq -sc '[.[] | select(.operation == "native_summary")] | length')"
complete_count="$(json_log | jq -sc '[.[] | select(.operation == "native_summary" and .data.status == "complete")] | length')"
json_log | jq -se 'any(.[]; .operation == "native_summary" and .data.output_name == "HEADLESS-1")' >/dev/null
json_log | jq -se 'any(.[]; .operation == "native_summary" and .data.output_name == "HEADLESS-2")' >/dev/null
json_log | jq -se 'any(.[]; .operation == "native_summary" and (.data.frame_work | type) == "object")' >/dev/null
json_log | jq -se '
  [ .[] | select(
      .operation == "native_editor_input_applied"
      and .data.run_id == "nested-editor-scenario"
      and .data.changed == true)
    | .data.action ]
  == ["canvas_output_moved", "canvas_visibility_none", "canvas_visibility_all", "canvas_deleted"]
' >/dev/null
json_log | jq -se '
  [ .[] | select(
      .operation == "native_editor_input_applied"
      and .data.run_id == "nested-editor-scenario") ] as $actions
  | [ .[] | select(.operation == "native_editor_projection_received") ] as $projections
  | ($actions | map(select(.data.action == "canvas_output_moved")) | first) as $moved
  | ($actions | map(select(.data.action == "canvas_visibility_none")) | first) as $hidden
  | ($actions | map(select(.data.action == "canvas_visibility_all")) | first) as $shown
  | ($actions | map(select(.data.action == "canvas_deleted")) | first) as $deleted
  | [ .[] | select(.operation == "native_editor_painted") ] as $paints
  | (($actions | length) == 4
    and all($actions[]; . as $action | any($projections[]; .data.revision == $action.data.revision))
    and any($projections[];
        .data.revision == $moved.data.revision
        and .data.output == "HEADLESS-2"
        and any(.data.canvases[]; .id == "wayland-status" and .output == "HEADLESS-2"))
    and all($projections[] | select(.data.revision == $hidden.data.revision);
        all(.data.canvases[]; .id != "wayland-status"))
    and any($projections[];
        .data.revision == $shown.data.revision
        and .data.output == "HEADLESS-2"
        and any(.data.canvases[]; .id == "wayland-status"))
    and all($projections[] | select(.data.revision == $deleted.data.revision);
        all(.data.canvases[]; .id != "wayland-status"))
    and all($actions[]; . as $action
      | all($projections[] | select(.data.revision == $action.data.revision); . as $accepted
        | any($paints[];
            .data.output == $accepted.data.output
            and .data.revision >= $action.data.revision
            and .timestamp_unix_us >= $accepted.timestamp_unix_us))))
' >/dev/null
json_log | jq -se 'any(.[]; .operation == "native_summary" and (.data.frame_work.frames | type) == "array")' >/dev/null
json_log | jq -se 'any(.[]; .operation == "native_surface_unmap")' >/dev/null
json_log | jq -se '
  [ .[] | select(
      .operation == "native_surface_visibility"
      and .data.canvas_id == "wayland-selection") ] as $visibility
  | (reduce $visibility[] as $event
      ({phase: 0};
       if .phase == 0 and $event.data.active == true then .phase = 1
       elif .phase == 1 and $event.data.active == false then .phase = 2
       elif .phase == 2 and $event.data.active == true then .phase = 3
       else . end)) as $progress
  | [ $visibility[] | select(.data.active == true) ] as $active
  | $progress.phase == 3
    and ($active | map(.data.run_id) | unique | length) == 1
' >/dev/null
json_log | jq -se '
  [ .[] | select(
      .operation == "native_surface_transition"
      and .data.canvas_id == "wayland-selection"
      and .data.painted == true
      and .data.surface_after == "mapped") ]
  | length >= 2 and (map(.data.run_id) | unique | length) == 1
' >/dev/null
json_log | jq -se '
  any(.[];
      .operation == "native_surface_transition"
      and .data.canvas_id == "wayland-selection"
      and .data.unmapped == true
      and .data.renderer_active_before == true
      and .data.renderer_active_after == false)
  and any(.[];
      .operation == "native_surface_transition"
      and .data.canvas_id == "wayland-selection"
      and .data.configured == true
      and .data.painted == true
      and .data.renderer_active_before == false
      and .data.renderer_active_after == true)
' >/dev/null
remap_latency_us="$(json_log | jq -ser '
  . as $records
  | [ $records[] | select(
      .operation == "native_surface_transition"
      and .data.canvas_id == "wayland-selection"
      and .data.visibility_changed == true
      and .data.visible == true) ] | last as $shown
  | [ $records[] | select(
      .operation == "native_surface_transition"
      and .data.canvas_id == "wayland-selection"
      and .data.run_id == $shown.data.run_id
      and .data.painted == true
      and .timestamp_unix_us >= $shown.timestamp_unix_us) ] | first as $painted
  | if $shown == null or $painted == null then null
    else $painted.timestamp_unix_us - $shown.timestamp_unix_us end
')"
test "$remap_latency_us" != "null"
test "$remap_latency_us" -ge 0
test "$remap_latency_us" -lt 250000
json_log | jq -se '
  . as $records
  | [$records[] | select(
      .operation == "native_surface_visibility"
      and .data.canvas_id == "wayland-selection"
      and .data.active == true) | .data.run_id] | last as $run_id
  | any($records[];
      .operation == "native_summary"
      and .data.run_id == $run_id
      and .data.canvas_id == "wayland-selection"
      and .data.skin_runtime_create_count == 1)
' >/dev/null
json_log | jq -se 'all(.[]; .operation != "native_canvas_failed")' >/dev/null
json_log | jq -se 'any(.[]; .operation == "nested_wayland_scenario" and .action == "pointer-drag-injected")' >/dev/null
test "$summary_count" -eq "$complete_count"
test "$complete_count" -ge 4

json_log | jq -se '
  [.[] | select(.operation == "native_projection_rebuilt")] as $modes
  | ([$modes[] | select(.data.editing == false)] | first) as $closed
  | $closed != null
    and any($modes[];
      .data.session_id == $closed.data.session_id
      and .data.editing == true
      and .data.revision > $closed.data.revision
      and .timestamp_unix_us > $closed.timestamp_unix_us)
' >/dev/null

pointer_started_us="$(json_log | jq -sc '[.[] | select(.operation == "nested_wayland_scenario" and .action == "pointer-input-started")] | first | .timestamp_unix_us')"
test "$pointer_started_us" != "null"
sample="$(json_log | jq -sc --argjson pointer_started_us "$pointer_started_us" '
  [.[] | select(
    .operation == "native_editor_input_applied"
    and .data.changed == true
    and .data.run_id != "nested-editor-scenario"
    and .timestamp_unix_us >= $pointer_started_us)] as $actions
  | [.[] | select(.operation == "native_editor_projection_received")] as $receipts
  | [.[] | select(.operation == "native_editor_painted")] as $paints
  | [$actions[] | . as $applied
    | [$receipts[] | select(
        .data.session_id == $applied.data.session_id
        and .data.revision == $applied.data.revision
        and .timestamp_unix_us >= $applied.timestamp_unix_us)][] as $received
    | [$paints[] | select(
        .data.output == $received.data.output
        and .data.session_id == $applied.data.session_id
        and .data.revision >= $applied.data.revision
        and .timestamp_unix_us >= $received.timestamp_unix_us)] | first as $painted
    | select($painted != null)
    | {applied:$applied,received:$received,painted:$painted}]
  | sort_by(.painted.timestamp_unix_us - .applied.timestamp_unix_us)
  | first
')"
if [[ "$sample" == "null" ]]; then
  echo "nested virtual-pointer input did not propagate through a stage projection and paint" >&2
  exit 1
fi
applied_us="$(jq -r '.applied.timestamp_unix_us' <<<"$sample")"
painted_us="$(jq -r '.painted.timestamp_unix_us' <<<"$sample")"
input_to_paint_us="$((painted_us - applied_us))"
test "$input_to_paint_us" -ge 0
test "$input_to_paint_us" -lt 250000

json_log | jq -se '
  [.[] | select(.operation == "native_summary" and .data.status == "complete" and (.data.effective_paint_hz // 0) >= 55)]
  | map(.data.output_name) | unique
  | index("HEADLESS-1") != null and index("HEADLESS-2") != null
' >/dev/null
json_log | jq -se '
  ["dioxus_poll", "projection_rebuild", "canvas_config", "package_open", "package_clone",
   "wasm_runtime_create", "skin_input", "wasm_render", "json_tree", "tree_reconciliation",
   "resource_lookup", "resource_decode", "blitz_layout", "scene", "gpu_present", "surface_commit"] as $required
  | [.[] | select(.operation == "native_summary" and .data.status == "complete" and (.data.frame_work.frames | length) > 0)]
  | length >= 2 and all(.[].data.frame_work.frames[]; (($required - (.phases | keys)) | length) == 0)
' >/dev/null
printf 'nested input-to-paint latency: %s us\n' "$input_to_paint_us"
printf 'nested display-remap latency: %s us\n' "$remap_latency_us"
