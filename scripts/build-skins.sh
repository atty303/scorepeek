#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
output="$root/target/skins"
staging="$root/target/skin-package-staging"
trap 'rm -rf "$staging"' EXIT
rm -rf "$staging"
mkdir -p "$output" "$staging"

scoper_probe=$(printf '@media(min-width:1px){.inside,.other{color:red}}.after{color:blue}' | awk -f "$root/scripts/scope-skin-css.awk")
if [[ "$scoper_probe" != '@media(min-width:1px){.scorepeek-skin-scope .inside,.scorepeek-skin-scope .other{color:red}}.scorepeek-skin-scope .after{color:blue}' ]]; then
  echo "skin CSS scoper self-test failed" >&2
  exit 1
fi
scoper_probe=$(printf '.a:is(.b,.c),[data-x="a,b"]{content:"}"}.after{color:blue}' | awk -f "$root/scripts/scope-skin-css.awk")
if [[ "$scoper_probe" != '.scorepeek-skin-scope .a:is(.b,.c),.scorepeek-skin-scope [data-x="a,b"]{content:"}"}.scorepeek-skin-scope .after{color:blue}' ]]; then
  echo "skin CSS scoper quote or selector-list self-test failed" >&2
  exit 1
fi

package() {
  local skin_dir=$1
  local name
  name=$(basename "$skin_dir")
  local crate="scorepeek-skin-$name"
  local work="$staging/$name"
  local module="${crate//-/_}.wasm"
  local shared=false
  if [[ -f "$skin_dir/skin.build.toml" ]] && grep -Eq '^shared[[:space:]]*=[[:space:]]*true[[:space:]]*$' "$skin_dir/skin.build.toml"; then
    shared=true
  fi

  cargo build --locked --release --target wasm32-unknown-unknown -p "$crate"
  mkdir -p "$work"
  cp "$skin_dir/skin.toml" "$work/skin.toml"
  cp "$root/target/wasm32-unknown-unknown/release/$module" "$work/skin.wasm"
  cp "$skin_dir/preview.png" "$work/preview.png"
  if [[ -f "$skin_dir/preview.webm" ]]; then
    cp "$skin_dir/preview.webm" "$work/preview.webm"
  fi
  if [[ "$shared" == true ]]; then
    cp "$root"/skins/shared/resources/* "$work/"
  fi
  if compgen -G "$skin_dir/resources/*" >/dev/null; then
    cp "$skin_dir"/resources/* "$work/"
  fi

  bash "$root/skins/shared/tools/compose-css.bash" "$root" "$skin_dir" "$work/skin.css"
  rm -f "$output/$name.zip"
  (cd "$work" && zip -q -X -9 "$output/$name.zip" ./*)
}

found=false
for manifest in "$root"/skins/*/skin.toml; do
  [[ -f "$manifest" ]] || continue
  found=true
  package "$(dirname "$manifest")"
done
if [[ "$found" != true ]]; then
  echo "no skins/*/skin.toml packages found" >&2
  exit 1
fi
