#!/usr/bin/env bash
set -euo pipefail

output="${1:-}"
if [[ "$output" != "library" && "$output" != "resource-dir" ]]; then
  echo "usage: $0 library|resource-dir" >&2
  exit 2
fi

libclang_inventory="$(ldconfig -p 2>/dev/null)"
best_major=-1
best_library=
best_resource_dir=

while IFS= read -r inventory_line; do
  if [[ "$inventory_line" =~ libclang\.so\.([0-9]+)[^[:space:]]*[[:space:]]+\(libc6,x86-64\)[[:space:]]+=\>[[:space:]]+(.+)$ ]]; then
    major="${BASH_REMATCH[1]}"
    library="${BASH_REMATCH[2]}"
    for resource_dir in "/usr/lib/clang/$major" "/usr/lib64/clang/$major"; do
      if (( major > best_major )) \
        && [[ -f "$library" ]] \
        && [[ -f "$resource_dir/include/stdbool.h" ]]; then
        best_major="$major"
        best_library="$library"
        best_resource_dir="$resource_dir"
      fi
    done
  fi
done <<<"$libclang_inventory"

if [[ -z "$best_library" || -z "$best_resource_dir" ]]; then
  echo "no x86-64 shared libclang has matching host resource headers" >&2
  exit 2
fi

case "$output" in
  library) printf '%s\n' "$best_library" ;;
  resource-dir) printf '%s\n' "$best_resource_dir" ;;
esac
