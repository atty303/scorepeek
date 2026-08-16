#!/usr/bin/env bash

acquire_catalog_schedule_mode_lock() {
  local runtime_dir=${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}
  case "$runtime_dir" in
    /*) ;;
    *)
      echo "XDG_RUNTIME_DIR must be an absolute path" >&2
      return 2
      ;;
  esac

  local lock_directory="$runtime_dir/scorepeek"
  install -d -m0700 -- "$lock_directory"
  exec {SCOREPEEK_SCHEDULE_MODE_LOCK_FD}>"$lock_directory/catalog-schedule-mode.lock"
  chmod 0600 -- "$lock_directory/catalog-schedule-mode.lock"
  flock "$SCOREPEEK_SCHEDULE_MODE_LOCK_FD"
}
