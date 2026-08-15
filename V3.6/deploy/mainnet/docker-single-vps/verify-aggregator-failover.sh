#!/usr/bin/env bash
set -euo pipefail

container=xhub-v36-monitor-c
endpoint='http://127.0.0.1:18744/api/v3.6/monitor-aggregate?protocol_version=0x0360'

docker pause "$container" >/dev/null
trap 'docker unpause xhub-v36-monitor-c >/dev/null 2>&1 || true' EXIT
echo "status=MONITOR_C_PAUSED"

found=false
for _ in $(seq 1 16); do
  body=$(curl --silent "$endpoint" || true)
  if printf '%s' "$body" | grep -q '"status":"DEGRADED"'; then
    printf '%s\n' "$body"
    found=true
    break
  fi
  sleep 5
done
test "$found" = true

docker unpause "$container" >/dev/null
trap - EXIT
echo "status=MONITOR_C_UNPAUSED"

recovered=false
for _ in $(seq 1 16); do
  body=$(curl --silent "$endpoint" || true)
  if printf '%s' "$body" | grep -q '"status":"READY"' \
    && printf '%s' "$body" | grep -q '"agreeing_count":3'; then
    printf '%s\n' "$body"
    recovered=true
    break
  fi
  sleep 5
done
test "$recovered" = true
echo "status=AGGREGATOR_RECOVERED"
