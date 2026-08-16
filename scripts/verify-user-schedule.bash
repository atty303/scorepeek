#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
service="$repository_root/contrib/systemd/user/scorepeek-catalog-sync.service"
timer="$repository_root/contrib/systemd/user/scorepeek-catalog-sync.timer"
verification_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-schedule-verify.XXXXXXXX")

cleanup() {
  case "$verification_root" in
    "${TMPDIR:-/tmp}"/scorepeek-schedule-verify.*)
      rm -rf -- "$verification_root"
      ;;
  esac
}
trap cleanup EXIT

verification_service="$verification_root/scorepeek-catalog-sync.service"
verification_timer="$verification_root/scorepeek-catalog-sync.timer"
cp -- "$service" "$verification_service"
cp -- "$timer" "$verification_timer"
verify_executable=$(realpath -- "$(command -v systemd-analyze)")
sed -i "s|^ExecStart=.*|ExecStart=$verify_executable --version|" "$verification_service"

systemd-analyze --user verify "$verification_service" "$verification_timer"

grep -Fqx 'Type=oneshot' "$service"
grep -Fqx 'Environment="XDG_DATA_HOME=%h/.local/share"' "$service"
grep -Fqx 'Environment="XDG_CACHE_HOME=%h/.cache"' "$service"
grep -Fqx 'ExecStart=%h/.local/bin/scorepeek catalog sync' "$service"
grep -Fqx 'TimeoutStartSec=10min' "$service"
grep -Fqx 'UMask=0077' "$service"
grep -Fqx 'OnCalendar=daily' "$timer"
grep -Fqx 'RandomizedDelaySec=6h' "$timer"
grep -Fqx 'Persistent=true' "$timer"
grep -Fqx 'Unit=scorepeek-catalog-sync.service' "$timer"
