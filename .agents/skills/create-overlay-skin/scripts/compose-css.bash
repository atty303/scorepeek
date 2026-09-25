#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: compose-css.bash REPOSITORY_ROOT SKIN_DIRECTORY OUTPUT" >&2
  exit 2
fi

root=$1
skin_dir=$2
output=$3
if [[ ! -f "$skin_dir/theme.css" ]]; then
  echo "missing skin theme.css: $skin_dir" >&2
  exit 1
fi

sed 's#/skins/##g' "$skin_dir/theme.css" |
  sed 's@/\*[^*]*\*/@@g' |
  awk -f "$root/scripts/scope-skin-css.awk" > "$output"
echo '.scorepeek-skin-scope,.scorepeek-skin-scope *{pointer-events:none}' >> "$output"
