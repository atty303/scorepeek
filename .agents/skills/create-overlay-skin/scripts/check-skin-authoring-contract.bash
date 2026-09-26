#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 || ! -f "$1/skin.toml" || ( $# -eq 2 && "$2" != --still ) ]]; then
  echo "usage: check-skin-authoring-contract.bash SKIN_DIRECTORY [--still]" >&2
  exit 2
fi

manifest=$1/skin.toml
taplo lint "$manifest"
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
  if [[ "$required" == animated && $# -eq 2 ]]; then
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
if [[ $# -eq 2 ]]; then
  echo "authoring background modes and six widget defaults verified; background_off_value=$off_value background_motion=still"
else
  echo "authoring background modes and six widget defaults verified; background_off_value=$off_value background_motion=animated"
fi
