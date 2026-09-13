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
  "${2:?missing virtual pointer executable}"
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
  SCOREPEEK_PRESERVE_XDG_RUNTIME_DIR=1 scripts/with-isolated-skins.sh target/release/examples/visual_wayland "$overlay_config" 15 --integration-fixture
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
    rg 'pointer-drag|nested_output_layout|native_editor_input|native_editor_action_reduced|native_projection_rebuilt|native_canvas_failed|status":"failed|error' "$log" | tail -n 200 || true
    rg 'nested_output_layout' "$log" || true
    json_log | jq -sc '
      [.[]|select(.operation=="native_editor_input_applied" and .data.run_id=="nested-editor-scenario")] as $actions
      | [.[]|select(.operation=="native_editor_projection_received")] as $projections
      | [.[]|select(.operation=="native_editor_painted")] as $paints
      | [$actions[] | . as $action
        | {action:.data.action,revision:.data.revision,changed:.data.changed,
           receipts:[$projections[]|select(.data.revision==$action.data.revision)
             | . as $accepted
             | {output:.data.output,canvases:[.data.canvases[].id],
                painted:any($paints[];.data.output==$accepted.data.output and .data.revision >= $action.data.revision and .timestamp_unix_us >= $accepted.timestamp_unix_us)}]}]
    ' || true
    json_log | jq -c 'select(.operation == "native_summary") | {operation,data:{run_id:.data.run_id,output_name:.data.output_name,status:.data.status,effective_paint_hz:.data.effective_paint_hz,paint_count:.data.paint_count}}' || true
    tail -n 200 "$log" | cut -c1-2000 || true
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
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --exercise-input "$repo_root/target/release/examples/virtual_pointer"
exec "$repo_root/scripts/test-overlay-wayland-nested.bash" --exit-compositor-after 30
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
  [ .[] | select(.operation == "native_editor_input_applied" and .data.run_id == "nested-editor-scenario") ] as $actions
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
