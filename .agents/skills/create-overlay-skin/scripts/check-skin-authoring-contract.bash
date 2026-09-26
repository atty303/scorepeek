#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 3 || ! -f "$1/skin.toml" ]]; then
  echo "usage: check-skin-authoring-contract.bash SKIN_DIRECTORY [--still] [--existing]" >&2
  exit 2
fi
still=false
existing_skin=false
for option in "${@:2}"; do
  case "$option" in
    --still) still=true ;;
    --existing) existing_skin=true ;;
    *) echo "unknown option: $option" >&2; exit 2 ;;
  esac
done

manifest=$1/skin.toml
root=$(cd "$(dirname "$0")/../../../.." && pwd)
taplo lint "$manifest"
skin_id=$(taplo get -f "$manifest" id)
if [[ -z "$skin_id" ]]; then
  echo "missing skin ID" >&2
  exit 1
fi
# An isolated authoring checkout can remove old skin files from its worktree
# without removing their committed IDs. Read only manifest IDs from HEAD here;
# old designs and shared implementation are not production inputs.
manifest_path=$(realpath "$manifest")
manifest_rel=$(realpath --relative-to="$root" "$manifest_path")
for existing in "$root"/skins/*/skin.toml; do
  [[ -f "$existing" ]] || continue
  [[ $(realpath "$existing") == "$manifest_path" ]] && continue
  if [[ $(taplo get -f "$existing" id) == "$skin_id" ]]; then
    echo "skin ID already used by $existing: $skin_id" >&2
    exit 1
  fi
done
while IFS= read -r tracked; do
  if [[ "$tracked" == "$manifest_rel" ]]; then
    if [[ "$existing_skin" == true ]] &&
      git -C "$root" ls-files --error-unmatch -- "$tracked" >/dev/null 2>&1; then
      committed_id=$(taplo get -f <(git -C "$root" show "HEAD:$tracked") id)
      if [[ "$skin_id" != "$committed_id" ]]; then
        echo "existing skin changed its committed ID at $tracked" >&2
        exit 1
      fi
      continue
    fi
    echo "skin directory already committed at $tracked; choose a new slug" >&2
    exit 1
  fi
  if [[ $(taplo get -f <(git -C "$root" show "HEAD:$tracked") id) == "$skin_id" ]]; then
    echo "skin ID already committed at $tracked: $skin_id" >&2
    exit 1
  fi
done < <(git -C "$root" ls-tree -r --name-only HEAD -- skins | rg '^skins/[^/]+/skin\.toml$' || true)
type=$(taplo get -f "$manifest" canvas_properties.background.type)
if [[ "$type" != enum ]]; then
  echo "canvas background must be an enum" >&2
  exit 1
fi
modes=$(taplo get -f "$manifest" canvas_properties.background.values)
off_value=
for candidate in none off; do
  if [[ $'\n'"$modes"$'\n' == *$'\n'"$candidate"$'\n'* ]]; then
    off_value=$candidate
    break
  fi
done
if [[ -z "$off_value" ]]; then
  echo "missing authoring no-background mode: none or off" >&2
  exit 1
fi
for required in static animated; do
  if [[ "$required" == animated && "$still" == true ]]; then
    if [[ $'\n'"$modes"$'\n' == *$'\n'animated$'\n'* ]]; then
      echo "still concept must not expose an ineffective animated mode" >&2
      exit 1
    fi
    continue
  fi
  if [[ $'\n'"$modes"$'\n' != *$'\n'"$required"$'\n'* ]]; then
    echo "missing authoring background mode: $required" >&2
    exit 1
  fi
done
for kind in status selection score history-list history-graph empty; do
  width=$(taplo get -f "$manifest" "widget_defaults.$kind.width")
  height=$(taplo get -f "$manifest" "widget_defaults.$kind.height")
  if [[ ! "$width" =~ ^[1-9][0-9]*$ || ! "$height" =~ ^[1-9][0-9]*$ ]]; then
    echo "missing positive widget default size: $kind" >&2
    exit 1
  fi
done
if [[ "$existing_skin" == true ]]; then
  identity_result="existing skin identity checked"
else
  identity_result="new skin ID and slug available"
fi
if [[ "$still" == true ]]; then
  echo "$identity_result; authoring background modes and six widget defaults verified; background_off_value=$off_value background_motion=still"
else
  echo "$identity_result; authoring background modes and six widget defaults verified; background_off_value=$off_value background_motion=animated"
fi
