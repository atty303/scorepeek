#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: compose-css.bash REPOSITORY_ROOT SKIN_DIRECTORY OUTPUT" >&2
  exit 2
fi

root=$1
skin_dir=$2
output=$3
shared=false
if [[ -f "$skin_dir/skin.build.toml" ]] && grep -Eq '^shared[[:space:]]*=[[:space:]]*true[[:space:]]*$' "$skin_dir/skin.build.toml"; then
  shared=true
fi

{
  if [[ "$shared" == true ]]; then
    echo "@font-face{font-family:Oxanium;src:url('Oxanium.ttf')}@font-face{font-family:Orbitron;src:url('Orbitron.ttf')}@font-face{font-family:Rajdhani;src:url('Rajdhani-SemiBold.ttf')}"
    for source in base rich composition motion; do
      sed 's#/skins/##g' "$root/skins/shared/styles/$source.css"
    done
  fi
  sed 's#/skins/##g' "$skin_dir/theme.css"
} | sed 's@/\*[^*]*\*/@@g' | awk -f "$root/scripts/scope-skin-css.awk" > "$output"
echo '.scorepeek-skin-scope,.scorepeek-skin-scope *{pointer-events:none}' >> "$output"
