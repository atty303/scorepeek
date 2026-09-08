#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
output="$root/target/skins"
staging="$root/target/skin-package-staging"
trap 'rm -rf "$staging"' EXIT
rm -rf "$staging"
mkdir -p "$output" "$staging"
cargo build --locked --release --target wasm32-unknown-unknown -p scorepeek-skin-guest-core
wasm="$root/target/wasm32-unknown-unknown/release/scorepeek_skin_guest_core.wasm"
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
  local name=$1
  local work="$staging/$name"
  mkdir -p "$work"
  cp "$root/skins/$name/skin.toml" "$work/skin.toml"
  cp "$wasm" "$work/skin.wasm"
  cp "$root/docs/design/overlay-canvas/$name.png" "$work/preview.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/$name-background.png" "$work/$name-background.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/$name-frame.png" "$work/$name-frame.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/type-$name.png" "$work/type-$name.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/labels-$name.png" "$work/labels-$name.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/Oxanium.ttf" "$work/Oxanium.ttf"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/Orbitron.ttf" "$work/Orbitron.ttf"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/Rajdhani-SemiBold.ttf" "$work/Rajdhani-SemiBold.ttf"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/OFL.txt" "$work/Oxanium-OFL.txt"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/orbitron-OFL.txt" "$work/Orbitron-OFL.txt"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/rajdhani-OFL.txt" "$work/Rajdhani-OFL.txt"
  if [[ "$name" == result-aurora ]]; then
    cp "$root/crates/scorepeek-overlay-ui/assets/skins/result-aurora-header.png" "$work/result-aurora-header.png"
  fi
  {
    echo "@font-face{font-family:Oxanium;src:url('Oxanium.ttf')}@font-face{font-family:Orbitron;src:url('Orbitron.ttf')}@font-face{font-family:Rajdhani;src:url('Rajdhani-SemiBold.ttf')}"
    sed 's#/skins/##g' "$root/crates/scorepeek-overlay-ui/styles/base.css"
    sed 's#/skins/##g' "$root/crates/scorepeek-overlay-ui/styles/$name.css"
    sed 's#/skins/##g' "$root/crates/scorepeek-overlay-ui/styles/rich.css"
    sed 's#/skins/##g' "$root/crates/scorepeek-overlay-ui/styles/composition.css"
    echo '@keyframes skin-background-opacity{0%,100%{opacity:.3}50%{opacity:.85}}@keyframes skin-energy-opacity{0%,100%{opacity:.06}50%{opacity:.16}}@keyframes skin-energy-left{0%,100%{left:-5%}50%{left:1%}}@keyframes skin-glint-left{0%{left:-20%}100%{left:100%}}@keyframes skin-full-combo-opacity{0%,100%{opacity:.82}50%{opacity:1}}@keyframes skin-ex-hard-opacity{0%,100%{opacity:.82}50%{opacity:1}}@keyframes skin-active-lamp-opacity{0%,100%{opacity:.68}50%{opacity:1}}@keyframes skin-processing-lamp-opacity{0%,100%{opacity:.4}50%{opacity:1}}'
    echo '.canvas-background[data-motion=animated] .canvas-background-light{animation:skin-background-opacity 16s ease-in-out infinite}.skin-energy{animation:skin-energy-opacity 7s ease-in-out infinite,skin-energy-left 11s ease-in-out infinite}.status-widget .skin-glint{animation:skin-glint-left 6s linear infinite}.selection-widget .skin-glint{animation:skin-glint-left 6s linear -.84s infinite}.score-widget .skin-glint{animation:skin-glint-left 6s linear -1.68s infinite}.history-list-widget .skin-glint{animation:skin-glint-left 6s linear -2.52s infinite}.history-graph-widget .skin-glint{animation:skin-glint-left 6s linear -3.36s infinite}[data-clear=full-combo]{animation:skin-full-combo-opacity .72s ease-in-out infinite}[data-clear=ex-hard]{animation:skin-ex-hard-opacity 1.1s ease-in-out infinite}.lamp[data-state=active],.lamp[data-state=persisted]{animation:skin-active-lamp-opacity 1.8s ease-in-out infinite}.lamp[data-state=processing]{animation:skin-processing-lamp-opacity .8s ease-in-out infinite}@media(prefers-reduced-motion:reduce){.skin-energy,.skin-glint,.canvas-background-light,[data-clear],.lamp{animation:none!important}}'
  } | sed 's@/\*[^*]*\*/@@g' | awk -f "$root/scripts/scope-skin-css.awk" > "$work/skin.css"
  echo '.scorepeek-skin-scope,.scorepeek-skin-scope *{pointer-events:none}' >> "$work/skin.css"
  rm -f "$output/$name.zip"
  (cd "$work" && zip -q -X -9 "$output/$name.zip" ./*)
}

package cyan-system
package result-aurora
package dj-blackbox
