#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source "$repository_root/scripts/catalog-schedule-common.bash"
acquire_catalog_schedule_mode_lock

load_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=LoadState --value)
if [[ "$load_state" != not-found ]]; then
  systemctl --user disable --now scorepeek-catalog-sync.timer
fi

for unit in \
  scorepeek-catalog-sync.service \
  scorepeek-catalog-sync-transient.timer \
  scorepeek-catalog-sync-transient.service; do
  load_state=$(systemctl --user show "$unit" --property=LoadState --value)
  if [[ "$load_state" != not-found ]]; then
    systemctl --user stop "$unit"
  fi
done

for unit in scorepeek-catalog-sync.timer scorepeek-catalog-sync-transient.timer; do
  load_state=$(systemctl --user show "$unit" --property=LoadState --value)
  active_state=$(systemctl --user show "$unit" --property=ActiveState --value)
  if [[ "$load_state" != not-found && "$active_state" != inactive && "$active_state" != failed ]]; then
    echo "$unit remains active after disable: $active_state" >&2
    exit 1
  fi
done
