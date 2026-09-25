#!/usr/bin/env bash
set -euo pipefail

test_root="$(mktemp -d /tmp/scorepeek-overlay-browser-test.XXXXXX)"
stop_fifo="$test_root/stop"
server_log="$test_root/server.log"
port="$(deno eval 'const listener = Deno.listen({ hostname: "127.0.0.1", port: 0 }); console.log(listener.addr.port); listener.close();')"
address="127.0.0.1:$port"
mkfifo "$stop_fifo"
exec 3<>"$stop_fifo"

cleanup() {
  status=$?
  if [[ -n "${test_pid:-}" ]]; then
    kill "$test_pid" 2>/dev/null || true
    wait "$test_pid" 2>/dev/null || true
  fi
  exec 3>&- || true
  if [[ -n "${server_pid:-}" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" || true
  fi
  if [[ "$status" -ne 0 ]]; then
    for log in "$server_log" "$test_root/role.stdout" "$test_root/role.stderr"; do
      if [[ -f "$log" ]]; then sed -n '1,240p' "$log" >&2; fi
    done
  fi
  find "$test_root" -depth -delete
  return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

scripts/with-isolated-skins.sh deno run -A scripts/overlay-fixture-host.deno.js "$test_root" "$address" <"$stop_fifo" 3>&- >"$server_log" 2>&1 &
server_pid=$!

for _ in {1..300}; do
  if curl --fail --silent --output /dev/null "http://$address/overlay"; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    sed -n '1,240p' "$server_log"
    exit 1
  fi
  sleep 0.1
done
curl --fail --silent --output /dev/null "http://$address/overlay"

SCOREPEEK_BROWSER_TEST_URL="http://$address" deno test -A tests/overlay-browser.test.js &
test_pid=$!
wait "$test_pid"
test_pid=
