#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: build-test-skin-package.bash OUTPUT_ZIP" >&2
  exit 2
fi

root=$(cd "$(dirname "$0")/.." && pwd)
output=$1
work=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-test-skin.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

sed \
  -e 's/^id = "dev.atty303.infinitas"$/id = "dev.atty303.test.skin.alternate"/' \
  -e 's/^name = "infinitas"$/name = "Alternate test skin"/' \
  "$root/skins/infinitas/skin.toml" > "$work/skin.toml"
cp "$root/target/wasm32-unknown-unknown/release/scorepeek_skin_infinitas.wasm" "$work/skin.wasm"
cp "$root/skins/infinitas/preview.png" "$work/preview.png"
cp "$root/skins/infinitas/preview.webm" "$work/preview.webm"
cp "$root"/skins/infinitas/resources/* "$work/"
bash "$root/.agents/skills/create-overlay-skin/scripts/compose-css.bash" \
  "$root" "$root/skins/infinitas" "$work/skin.css"
echo '.scorepeek-skin-scope .song-title{font-size:10px}' >> "$work/skin.css"
(cd "$work" && zip -q -X -9 "$output" ./*)
