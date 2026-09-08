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

package() {
  local name=$1
  local preview=$2
  local frame=$3
  local font=$4
  local license=$5
  local work="$staging/$name"
  mkdir -p "$work"
  cp "$root/skins/$name/skin.toml" "$work/skin.toml"
  cp "$root/skins/$name/skin.css" "$work/skin.css"
  cp "$wasm" "$work/skin.wasm"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/$preview" "$work/preview.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/$preview" "$work/background.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/skins/$frame" "$work/frame.png"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/$font" "$work/font.ttf"
  cp "$root/crates/scorepeek-overlay-ui/assets/fonts/$license" "$work/font-license.txt"
  rm -f "$output/$name.zip"
  (cd "$work" && zip -q -X -9 "$output/$name.zip" skin.toml skin.wasm skin.css preview.png background.png frame.png font.ttf font-license.txt)
}

package cyan-system cyan-system-background.png cyan-system-frame.png Oxanium.ttf OFL.txt
package result-aurora result-aurora-background.png result-aurora-frame.png Oxanium.ttf OFL.txt
package dj-blackbox dj-blackbox-background.png dj-blackbox-frame.png Rajdhani-SemiBold.ttf rajdhani-OFL.txt
