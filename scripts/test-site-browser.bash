#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_root="$(mktemp -d /tmp/scorepeek-site-browser-test.XXXXXX)"

cleanup() {
  local status=$?
  find "$test_root" -depth -delete
  return "$status"
}
trap cleanup EXIT

SCOREPEEK_SITE_ROOT="$root/site" \
  playwright test tests/site-browser.spec.js \
    --workers=1 \
    --reporter=line \
    --output="$test_root/playwright"
