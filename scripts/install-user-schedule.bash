#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source "$repository_root/scripts/catalog-schedule-common.bash"
home=${HOME:?HOME must be set}
config_home=${XDG_CONFIG_HOME:-"$home/.config"}
data_home=${XDG_DATA_HOME:-"$home/.local/share"}
cache_home=${XDG_CACHE_HOME:-"$home/.cache"}
service_path="$config_home/systemd/user/scorepeek-catalog-sync.service"
timer_path="$config_home/systemd/user/scorepeek-catalog-sync.timer"
temporary_service=
persistent_activation_started=false
persistent_activation_complete=false

cleanup() {
  local cleanup_failed=false
  if [[ "$persistent_activation_started" == true && "$persistent_activation_complete" != true ]]; then
    systemctl --user disable --now scorepeek-catalog-sync.timer >/dev/null 2>&1 || cleanup_failed=true
    systemctl --user stop scorepeek-catalog-sync.service >/dev/null 2>&1 || cleanup_failed=true
  fi
  if [[ -n "$temporary_service" ]]; then
    rm -f -- "$temporary_service"
  fi
  if [[ "$cleanup_failed" == true ]]; then
    echo "failed to roll back persistent catalog scheduling" >&2
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

case "$home" in
  /*) ;;
  *)
    echo "HOME must be an absolute path" >&2
    exit 2
    ;;
esac
case "$config_home" in
  /*) ;;
  *)
    echo "XDG_CONFIG_HOME must be an absolute path when set" >&2
    exit 2
    ;;
esac
for path in "$data_home" "$cache_home"; do
  case "$path" in
    /*) ;;
    *)
      echo "XDG data and cache directories must be absolute paths" >&2
      exit 2
      ;;
  esac
  if [[ "$path" == *$'\n'* || "$path" == *$'\r'* ]]; then
    echo "XDG data and cache directories must not contain newlines" >&2
    exit 2
  fi
done

systemd_environment() {
  local name=$1
  local value=$2
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//%/%%}
  printf 'Environment="%s=%s"\n' "$name" "$value"
}

require_inactive() {
  local unit=$1
  local load_state
  local active_state
  load_state=$(systemctl --user show "$unit" --property=LoadState --value)
  if [[ "$load_state" == not-found ]]; then
    return
  fi
  active_state=$(systemctl --user show "$unit" --property=ActiveState --value)
  if [[ "$active_state" != inactive && "$active_state" != failed ]]; then
    echo "$unit is active; run mise run catalog:schedule:systemd:disable before changing schedule mode" >&2
    exit 1
  fi
}

acquire_catalog_schedule_mode_lock
require_inactive scorepeek-catalog-sync-transient.timer
require_inactive scorepeek-catalog-sync-transient.service

cd -- "$repository_root"
cargo build --locked --release

install -Dm0755 \
  "$repository_root/target/release/scorepeek" \
  "$home/.local/bin/scorepeek"
install -d -m0755 -- "$(dirname -- "$service_path")"
temporary_service=$(mktemp "$(dirname -- "$service_path")/.scorepeek-catalog-sync.service.XXXXXXXX")
install -m0644 \
  "$repository_root/contrib/systemd/user/scorepeek-catalog-sync.service" \
  "$temporary_service"
{
  echo
  systemd_environment XDG_DATA_HOME "$data_home"
  systemd_environment XDG_CACHE_HOME "$cache_home"
} >>"$temporary_service"
mv -f -- "$temporary_service" "$service_path"
temporary_service=
install -Dm0644 \
  "$repository_root/contrib/systemd/user/scorepeek-catalog-sync.timer" \
  "$timer_path"

systemctl --user daemon-reload
for unit_path in "$service_path" "$timer_path"; do
  unit=$(basename -- "$unit_path")
  fragment=$(systemctl --user show "$unit" --property=FragmentPath --value)
  if [[ -z "$fragment" ]]; then
    systemctl --user link "$unit_path"
  elif [[ "$(realpath -- "$fragment")" != "$(realpath -- "$unit_path")" ]]; then
    echo "$unit is already loaded from a different path: $fragment" >&2
    exit 1
  fi
done
systemctl --user daemon-reload
for unit_path in "$service_path" "$timer_path"; do
  unit=$(basename -- "$unit_path")
  load_state=$(systemctl --user show "$unit" --property=LoadState --value)
  fragment=$(systemctl --user show "$unit" --property=FragmentPath --value)
  if [[ "$load_state" != loaded || "$(realpath -- "$fragment")" != "$(realpath -- "$unit_path")" ]]; then
    echo "$unit did not load from the installed path" >&2
    exit 1
  fi
done
persistent_activation_started=true
systemctl --user enable --now scorepeek-catalog-sync.timer
active_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=ActiveState --value)
unit_file_state=$(systemctl --user show scorepeek-catalog-sync.timer --property=UnitFileState --value)
if [[ "$active_state" != active || "$unit_file_state" != enabled ]]; then
  echo "persistent timer did not become active and enabled" >&2
  exit 1
fi
require_inactive scorepeek-catalog-sync-transient.timer
require_inactive scorepeek-catalog-sync-transient.service
persistent_activation_complete=true
systemctl --user show scorepeek-catalog-sync.timer \
  --property=ActiveState \
  --property=NextElapseUSecRealtime
