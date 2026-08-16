#!/usr/bin/env bash
set -euo pipefail

root=/opt/xhub-v3.6-test
token_file="$root/secrets/monitor-aggregate-api-token.txt"
container=xhub-v36-monitor-aggregate
endpoint=http://127.0.0.1:18744
protocol_version=0x0360
temporary_token_file=
old_token=

write_token() {
  local value=$1
  temporary_token_file=$(sudo mktemp "$root/secrets/.monitor-aggregate-token.XXXXXX")
  printf '%s\n' "$value" | sudo tee "$temporary_token_file" >/dev/null
  sudo chown 65532:65532 "$temporary_token_file"
  sudo chmod 0400 "$temporary_token_file"
  sudo mv "$temporary_token_file" "$token_file"
  temporary_token_file=
}

wait_for_healthy() {
  for _ in $(seq 1 30); do
    if [[ $(sudo docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' \
      "$container") == healthy ]]; then
      return 0
    fi
    sleep 2
  done
  return 1
}

rollback() {
  local exit_code=$?
  set +e
  if [[ -n "$temporary_token_file" ]]; then
    sudo rm -f "$temporary_token_file"
  fi
  if [[ -n "$old_token" ]]; then
    write_token "$old_token"
    sudo docker restart "$container" >/dev/null
    wait_for_healthy
  fi
  old_token=
  new_token=
  exit "$exit_code"
}
trap rollback ERR INT TERM

command -v curl >/dev/null
command -v jq >/dev/null
command -v openssl >/dev/null
sudo test -s "$token_file"
old_token=$(sudo cat "$token_file")
new_token=$(openssl rand -hex 32)
test -n "$new_token"
test "$new_token" != "$old_token"

write_token "$new_token"
sudo docker restart "$container" >/dev/null
wait_for_healthy

request_body=$(jq -nc \
  --arg protocol_version "$protocol_version" \
  '{protocol_version:$protocol_version,operator_id:"token-rotation-check",note:"authorization probe only"}')
url="$endpoint/api/v3.6/alerts/0/acknowledge"
old_status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --header "Authorization: Bearer $old_token" \
  --data "$request_body" \
  "$url")
new_status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --header "Authorization: Bearer $new_token" \
  --data "$request_body" \
  "$url")
test "$old_status" = 401
test "$new_status" = 404

status=$(curl --fail --silent \
  "$endpoint/api/v3.6/monitor-aggregate?protocol_version=$protocol_version")
jq -e '
  .production_ready == false and
  .spend_bundle_created == false and
  .broadcast_enabled == false and
  .broadcast_ready == false and
  .chain_broadcast == false
' <<<"$status" >/dev/null
if sudo docker logs "$container" 2>&1 | grep -Fq "$new_token"; then
  echo "rotated Bearer token appeared in aggregate logs" >&2
  false
fi

trap - ERR INT TERM
old_token=
new_token=
jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson old_token_http_status "$old_status" \
  --argjson new_token_http_status "$new_status" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-token-rotation-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    container:"xhub-v36-monitor-aggregate",
    health:"healthy",
    old_token_http_status:$old_token_http_status,
    new_token_http_status:$new_token_http_status,
    token_present_in_output:false,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
