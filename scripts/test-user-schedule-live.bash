#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
runtime_root=$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-schedule-live.XXXXXXXX")
unit="scorepeek-catalog-sync-live-$$"
manual_pid=
units_may_exist=false

cleanup() {
  local cleanup_failed=false
  if [[ -n "$manual_pid" ]] && kill -0 "$manual_pid" 2>/dev/null; then
    kill "$manual_pid" 2>/dev/null || true
    local deadline=$((SECONDS + 10))
    while kill -0 "$manual_pid" 2>/dev/null && (( SECONDS < deadline )); do
      sleep 0.1
    done
    if kill -0 "$manual_pid" 2>/dev/null; then
      kill -KILL "$manual_pid" 2>/dev/null || true
    fi
    wait "$manual_pid" 2>/dev/null || true
  fi
  if [[ "$units_may_exist" == true ]]; then
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
    echo "cleanup could not confirm stopped units; preserved $runtime_root for $unit" >&2
    return 1
  fi
  case "$runtime_root" in
    "${TMPDIR:-/tmp}"/scorepeek-schedule-live.*)
      rm -rf -- "$runtime_root"
      ;;
  esac
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

cd -- "$repository_root"
cargo build --locked --release

data_home="$runtime_root/data"
cache_home="$runtime_root/cache"
scheduled_stdout="$runtime_root/scheduled.stdout"
scheduled_stderr="$runtime_root/scheduled.stderr"
manual_stdout="$runtime_root/manual.stdout"
manual_stderr="$runtime_root/manual.stderr"
binary="$repository_root/target/release/scorepeek"
lock="$data_home/scorepeek/catalog/catalog-sync.lock"

units_may_exist=true
systemd-run --user \
  --unit="$unit" \
  --on-active=1s \
  --timer-property=AccuracySec=1us \
  --timer-property=RandomizedDelaySec=0 \
  --property=Type=oneshot \
  --property=UMask=0077 \
  --property=TimeoutStartSec=10min \
  --property="StandardOutput=file:$scheduled_stdout" \
  --property="StandardError=file:$scheduled_stderr" \
  --setenv="XDG_DATA_HOME=$data_home" \
  --setenv="XDG_CACHE_HOME=$cache_home" \
  "$binary" catalog sync >/dev/null

deadline=$((SECONDS + 120))
while :; do
  if [[ -e "$lock" ]] && ! flock -n "$lock" true; then
    break
  fi
  if (( SECONDS >= deadline )); then
    echo "scheduled sync did not acquire the writer lock within 120 seconds" >&2
    exit 1
  fi
  sleep 0.1
done

XDG_DATA_HOME="$data_home" XDG_CACHE_HOME="$cache_home" \
  "$binary" catalog sync >"$manual_stdout" 2>"$manual_stderr" &
manual_pid=$!
if flock -n "$lock" true; then
  echo "scheduled sync released the writer lock before the concurrent check" >&2
  exit 1
fi
if ! kill -0 "$manual_pid" 2>/dev/null; then
  echo "manual sync exited while the scheduled writer lock was held" >&2
  exit 1
fi

deadline=$((SECONDS + 600))
while :; do
  active_state=$(systemctl --user show "$unit.service" --property=ActiveState --value)
  case "$active_state" in
    inactive | failed)
      break
      ;;
  esac
  if (( SECONDS >= deadline )); then
    echo "scheduled sync did not finish within 10 minutes" >&2
    exit 1
  fi
  sleep 0.2
done

scheduled_result=$(systemctl --user show "$unit.service" --property=Result --value)
scheduled_status=$(systemctl --user show "$unit.service" --property=ExecMainStatus --value)
if [[ "$scheduled_result" != success || "$scheduled_status" != 0 ]]; then
  echo "scheduled sync failed: result=$scheduled_result status=$scheduled_status" >&2
  sed -n '1,20p' "$scheduled_stderr" >&2
  exit 1
fi

manual_status=0
wait "$manual_pid" || manual_status=$?
manual_pid=
if (( manual_status != 0 )); then
  echo "concurrent manual sync failed: status=$manual_status" >&2
  sed -n '1,20p' "$manual_stderr" >&2
  exit 1
fi

test ! -s "$scheduled_stderr"
test ! -s "$manual_stderr"
test -s "$scheduled_stdout"
test -s "$manual_stdout"
test "$(wc -l <"$scheduled_stdout")" -eq 1
test "$(wc -l <"$manual_stdout")" -eq 1
grep -Eq '^\{.*\}$' "$scheduled_stdout"
grep -Eq '^\{.*\}$' "$manual_stdout"

if cmp -s "$scheduled_stdout" "$manual_stdout"; then
  echo "schedule-triggered and concurrent manual catalog sync passed with unchanged source output"
else
  echo "schedule-triggered and concurrent manual catalog sync passed across a source output change"
fi
