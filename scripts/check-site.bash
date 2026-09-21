#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly site="$root/site"
readonly ja="$site/ja/index.html"
readonly en="$site/en/index.html"

required_files=(
  "$site/index.html"
  "$ja"
  "$en"
  "$site/assets/site.css"
  "$site/assets/scorepeek-logo-dark.png"
  "$site/assets/cyan-system.png"
  "$site/assets/result-aurora.png"
  "$site/assets/dj-blackbox.png"
)

for required in "${required_files[@]}"; do
  if [[ ! -f "$required" ]]; then
    printf 'landing page asset is missing: %s\n' "$required" >&2
    exit 1
  fi
done

cmp "$root/docs/assets/scorepeek-logo-dark.png" "$site/assets/scorepeek-logo-dark.png"
cmp "$root/skins/cyan-system/preview.png" "$site/assets/cyan-system.png"
cmp "$root/skins/result-aurora/preview.png" "$site/assets/result-aurora.png"
cmp "$root/skins/dj-blackbox/preview.png" "$site/assets/dj-blackbox.png"

extract_ids() {
  grep -oE 'id="[A-Za-z0-9_-]+"' "$1" | cut -d '"' -f 2
}

if ! diff -u <(extract_ids "$ja") <(extract_ids "$en"); then
  echo 'Japanese and English pages must keep the same structural IDs in the same order.' >&2
  exit 1
fi

for page in "$ja" "$en"; do
  grep -q '<!doctype html>' "$page"
  grep -q 'id="main-content"' "$page"
  grep -q 'id="how"' "$page"
  grep -q 'id="overlay"' "$page"
  grep -q 'id="use"' "$page"
  grep -q 'id="technology"' "$page"
  grep -q 'https://github.com/atty303/scorepeek' "$page"
  grep -q "connect-src 'none'" "$page"
  ! grep -Eqi '<script|analytics|googletag|segment\.com|plausible\.io' "$page"
  ! grep -Eqi '<(script|link)[^>]+(src|href)="https?://' "$page"

  while IFS= read -r asset; do
    [[ -f "$site/${asset#../}" ]] || {
      printf 'page references a missing local asset: %s in %s\n' "$asset" "$page" >&2
      exit 1
    }
  done < <(grep -oE '(src|href)="\.\./assets/[^"#]+' "$page" | cut -d '"' -f 2 | sort -u)
done

grep -q '<html lang="ja">' "$ja"
grep -q '<html lang="en">' "$en"
grep -q 'href="../en/"' "$ja"
grep -q 'href="../ja/"' "$en"
grep -q 'navigator.languages' "$site/index.html"
grep -q 'location.replace' "$site/index.html"

printf 'landing page structure and local assets are valid\n'
