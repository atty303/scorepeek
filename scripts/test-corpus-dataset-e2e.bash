#!/usr/bin/env bash
set -euo pipefail

test_root="$(mktemp -d "${TMPDIR:-/tmp}/scorepeek-corpus-dataset-e2e.XXXXXX")"
server_pid=""
stage="setup"

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if (( status != 0 )); then
    printf 'recording dataset E2E failed at stage: %s\n' "$stage" >&2
    if [[ -f "$test_root/rclone.log" ]]; then
      sed -n '1,160p' "$test_root/rclone.log" >&2
    fi
  fi
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid"
    wait "$server_pid" || true
  fi
  if [[ -f "$test_root/.scorepeek-e2e-owned" ]]; then
    rm -rf -- "$test_root"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

touch "$test_root/.scorepeek-e2e-owned"
private="$test_root/private"
server_root="$test_root/server"
bucket="$server_root/scorepeek-e2e"
mkdir -p "$private" "$bucket"
chmod 0700 "$private" "$server_root" "$bucket"

server_log="$test_root/rclone.log"
rclone serve s3 "$server_root" \
  --addr 127.0.0.1:0 \
  --auth-key scorepeek-e2e-access,scorepeek-e2e-secret-not-a-key \
  --config /dev/null \
  --log-level DEBUG >"$server_log" 2>&1 &
server_pid=$!

stage="server readiness"
endpoint=""
for _ in $(seq 1 200); do
  endpoint="$(sed -nE 's#.*Starting s3 server on \[(http://127\.0\.0\.1:[0-9]+)/\].*#\1#p' "$server_log" | head -n 1)"
  if [[ -n "$endpoint" ]]; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "rclone S3 server exited before readiness" >&2
    sed -n '1,120p' "$server_log" >&2
    exit 1
  fi
  sleep 0.05
done
if [[ -z "$endpoint" ]]; then
  echo "rclone S3 server did not report readiness" >&2
  exit 1
fi

capture_context="$private/capture-context.json"
stage="fixture preparation"
cat >"$capture_context" <<'JSON'
{"schema":"scorepeek-capture-context-v1","route":"portal_pipewire","environment_id":"synthetic-e2e","capture_adapter_id":"synthetic-matroska","capture_adapter_version":"v1","settings_revision":"multipart-e2e-v1"}
JSON

remote="$private/remote.json"
cat >"$remote" <<JSON
{"schema":"scorepeek-corpus-s3-remote-v1","url":"s3://scorepeek-e2e/dataset","region":"us-east-1","endpoint":"$endpoint","path_style":true,"allow_http_loopback":true}
JSON

padding="$private/padding.bin"
recording="$private/complete-run.mkv"
dd if=/dev/zero of="$padding" bs=1048576 count=9 status=none
ffmpeg -v error \
  -f lavfi \
  -i color=c=black:s=320x180:r=3:d=1 \
  -c:v ffv1 \
  -pix_fmt rgb24 \
  -attach "$padding" \
  -metadata:s:t mimetype=application/octet-stream \
  "$recording"
if (( $(stat -c %s "$recording") <= 8 * 1024 * 1024 )); then
  echo "synthetic recording did not cross the multipart threshold" >&2
  exit 1
fi

export AWS_ACCESS_KEY_ID=scorepeek-e2e-access
export AWS_SECRET_ACCESS_KEY=scorepeek-e2e-secret-not-a-key
export NO_PROXY=127.0.0.1,::1
export no_proxy="$NO_PROXY"
unset AWS_SESSION_TOKEN AWS_WEB_IDENTITY_TOKEN_FILE AWS_ROLE_ARN AWS_ROLE_SESSION_NAME
unset AWS_CONTAINER_CREDENTIALS_RELATIVE_URI AWS_CONTAINER_CREDENTIALS_FULL_URI
unset AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE

corpus=(cargo run --locked --quiet -p scorepeek-corpus --)
store="$private/store"
stage="recording import"
import_json="$("${corpus[@]}" recording import --store "$store" --capture-context "$capture_context" "$recording")"
recording_sha256="$(sed -nE 's/.*"recording_sha256":"([0-9a-f]{64})".*/\1/p' <<<"$import_json")"
if [[ -z "$recording_sha256" ]]; then
  echo "recording import did not return a recording digest" >&2
  exit 1
fi

stage="dataset seal"
seal_json="$("${corpus[@]}" dataset seal --store "$store" calibration-e2e)"
generation_sha256="$(sed -nE 's/.*"generation_sha256":"([0-9a-f]{64})".*/\1/p' <<<"$seal_json")"
if [[ -z "$generation_sha256" ]]; then
  echo "dataset seal did not return a generation digest" >&2
  exit 1
fi

stage="local verification"
"${corpus[@]}" dataset verify --store "$store" "$generation_sha256" >/dev/null
stage="initial remote push"
first_push="$("${corpus[@]}" dataset push --store "$store" --remote "$remote" "$generation_sha256")"
if [[ "$first_push" != *'"transferred_objects":6'* ]]; then
  echo "initial push did not transfer the complete generation" >&2
  exit 1
fi
stage="multipart protocol evidence"
if ! grep -Fq 'serve s3: initiate multipart upload' "$server_log" \
  || ! grep -Fq 'serve s3: put multipart upload' "$server_log" \
  || ! grep -Fq 'serve s3: complete multipart upload' "$server_log"; then
  echo "initial push did not exercise the multipart protocol" >&2
  exit 1
fi
stage="idempotent remote push"
second_push="$("${corpus[@]}" dataset push --store "$store" --remote "$remote" "$generation_sha256")"
if [[ "$second_push" != *'"transferred_objects":0'* || "$second_push" != *'"reused_objects":6'* ]]; then
  echo "repeated push did not reuse the verified generation" >&2
  exit 1
fi
stage="remote verification"
"${corpus[@]}" dataset remote-verify --store "$store" --remote "$remote" "$generation_sha256" >/dev/null

restored_store="$private/restored-store"
stage="remote pull"
"${corpus[@]}" dataset pull --store "$restored_store" --remote "$remote" "$generation_sha256" >/dev/null
stage="restored local verification"
"${corpus[@]}" dataset verify --store "$restored_store" "$generation_sha256" >/dev/null
stage="restored source comparison"
cmp "$recording" "$restored_store/content/$recording_sha256/source.media"

printf '{"schema":"scorepeek-corpus-dataset-e2e-v1","generation_sha256":"%s","recording_sha256":"%s"}\n' \
  "$generation_sha256" \
  "$recording_sha256"
