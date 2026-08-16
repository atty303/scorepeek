#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source "$repository_root/scripts/catalog-schedule-common.bash"
unit=scorepeek-catalog-sync-transient
home=${HOME:?HOME must be set}
data_home=${XDG_DATA_HOME:-"$home/.local/share"}
cache_home=${XDG_CACHE_HOME:-"$home/.cache"}
transient_activation_started=false
transient_activation_complete=false

cleanup() {
  local cleanup_failed=false
  if [[ "$transient_activation_started" == true && "$transient_activation_complete" != true ]]; then
    systemctl --user stop "$unit.timer" "$unit.service" >/dev/null 2>&1 || true
    for managed_unit in "$unit.timer" "$unit.service"; do
      if ! load_state=$(systemctl --user show "$managed_unit" --property=LoadState --value); then
        cleanup_failed=true
        continue
      fi
      if [[ "$load_state" == not-found ]]; then
        continue
      fi
      if ! active_state=$(systemctl --user show "$managed_unit" --property=ActiveState --value); then
        cleanup_failed=true
        continue
      fi
      if [[ "$active_state" != inactive && "$active_state" != failed ]]; then
        cleanup_failed=true
      fi
    done
  fi
  if [[ "$cleanup_failed" == true ]]; then
    echo "failed to roll back transient catalog scheduling" >&2
    return 1
  fi
}
on_exit() {
  local status=$?
  trap - EXIT
  if ! cleanup; then
    exit 1
  fi
  exit "$status"
}
trap on_exit EXIT

for path in "$data_home" "$cache_home"; do
  case "$path" in
    /*) ;;
    *)
      echo "XDG data and cache directories must be absolute paths" >&2
      exit 2
      ;;
  esac
done

acquire_catalog_schedule_mode_lock
persistent_load_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=LoadState --value)
if [[ "$persistent_load_state" != not-found ]]; then
  persistent_active_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=ActiveState --value)
  persistent_unit_file_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=UnitFileState --value)
  if [[ "$persistent_active_state" != inactive && "$persistent_active_state" != failed ]] ||
    [[ "$persistent_unit_file_state" == enabled || "$persistent_unit_file_state" == enabled-runtime ]]; then
    echo "persistent scheduling is active or enabled; run mise run catalog:schedule:systemd:disable before changing schedule mode" >&2
    exit 1
  fi
fi
persistent_service_load_state=$(systemctl --user show scorepeek-catalog-sync.service --property=LoadState --value)
if [[ "$persistent_service_load_state" != not-found ]]; then
  persistent_service_active_state=$(systemctl --user show scorepeek-catalog-sync.service --property=ActiveState --value)
  if [[ "$persistent_service_active_state" != inactive && "$persistent_service_active_state" != failed ]]; then
    echo "persistent catalog sync is running; run mise run catalog:schedule:systemd:disable before changing schedule mode" >&2
    exit 1
  fi
fi
transient_load_state=$(systemctl --user show "$unit.timer" --property=LoadState --value)
if [[ "$transient_load_state" != not-found ]]; then
  transient_existing_state=$(systemctl --user show "$unit.timer" --property=ActiveState --value)
  if [[ "$transient_existing_state" != inactive && "$transient_existing_state" != failed ]]; then
    echo "transient scheduling is already active; run mise run catalog:schedule:systemd:disable before restarting it" >&2
    exit 1
  fi
fi

cd -- "$repository_root"
cargo build --locked --release

transient_activation_started=true
systemd-run --user \
  --unit="$unit" \
  --on-calendar=daily \
  --timer-property=RandomizedDelaySec=6h \
  --timer-property=Persistent=false \
  --property=Type=oneshot \
  --property=UMask=0077 \
  --property=TimeoutStartSec=10min \
  --setenv="XDG_DATA_HOME=$data_home" \
  --setenv="XDG_CACHE_HOME=$cache_home" \
  "$repository_root/target/release/scorepeek" catalog sync
transient_active_state=$(systemctl --user show "$unit.timer" --property=ActiveState --value)
if [[ "$transient_active_state" != active ]]; then
  echo "transient timer did not become active" >&2
  exit 1
fi
transient_activation_complete=true
systemctl --user show "$unit.timer" \
  --property=ActiveState \
  --property=NextElapseUSecRealtime
