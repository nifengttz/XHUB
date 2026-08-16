#!/usr/bin/env bash
set -euo pipefail

aggregate_url=http://127.0.0.1:18744
protocol_version=0x0360
aggregate_container=xhub-v36-monitor-aggregate
fault_container=xhub-v36-monitor-c
overlay=/opt/xhub-v3.6-test/generated-compose.local/compose.readonly-monitors.yaml
database=/opt/xhub-v3.6-test/data/monitor-aggregate/alerts.sqlite3
token_file=/opt/xhub-v3.6-test/secrets/monitor-aggregate-api-token.txt
paused=false
response_file=$(mktemp)

cleanup() {
  rm -f "$response_file"
  if [[ "$paused" == true ]]; then
    sudo docker unpause "$fault_container" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

for command in curl jq stat; do
  command -v "$command" >/dev/null
done
sudo test -s "$token_file"
sudo test -s "$database"

assert_no_broadcast() {
  jq -e '
    .spend_bundle_created == false and
    .broadcast_enabled == false and
    .broadcast_ready == false and
    .chain_broadcast == false
  ' >/dev/null
}

alerts() {
  curl --fail --silent \
    "$aggregate_url/api/v3.6/alerts?protocol_version=$protocol_version&limit=100"
}

wait_for_aggregate() {
  local expected_status=$1
  local expected_agreeing=$2
  local body
  for _ in $(seq 1 30); do
    body=$(curl --silent \
      "$aggregate_url/api/v3.6/monitor-aggregate?protocol_version=$protocol_version" || true)
    if jq -e \
      --arg status "$expected_status" \
      --argjson agreeing "$expected_agreeing" \
      '.status == $status and .agreeing_count == $agreeing' \
      <<<"$body" >/dev/null 2>&1; then
      assert_no_broadcast <<<"$body"
      printf '%s' "$body"
      return 0
    fi
    sleep 5
  done
  echo "aggregate did not reach $expected_status with agreeing_count=$expected_agreeing" >&2
  return 1
}

wait_for_occurrence_increase() {
  local event_id=$1
  local prior_count=$2
  local body count
  for _ in $(seq 1 24); do
    body=$(alerts)
    assert_no_broadcast <<<"$body"
    count=$(jq -r --argjson event_id "$event_id" \
      '.events[] | select(.event_id == $event_id) | .occurrence_count' <<<"$body")
    if [[ -n "$count" ]] && (( count > prior_count )); then
      printf '%s' "$count"
      return 0
    fi
    sleep 5
  done
  echo "normal aggregate event was not deduplicated into a higher occurrence count" >&2
  return 1
}

wait_for_healthy() {
  local container=$1
  for _ in $(seq 1 30); do
    if [[ $(sudo docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' \
      "$container") == healthy ]]; then
      return 0
    fi
    sleep 2
  done
  echo "$container did not become healthy" >&2
  return 1
}

for container in \
  xhub-v36-monitor-a \
  xhub-v36-monitor-b \
  xhub-v36-monitor-c \
  "$aggregate_container"; do
  wait_for_healthy "$container"
done

baseline=$(wait_for_aggregate READY 3)
baseline_alerts=$(alerts)
assert_no_broadcast <<<"$baseline_alerts"
baseline_event_id=$(jq -r '.events[0].event_id' <<<"$baseline_alerts")
baseline_occurrence=$(jq -r '.events[0].occurrence_count' <<<"$baseline_alerts")
baseline_fingerprint=$(jq -r '.events[0].fingerprint' <<<"$baseline_alerts")
baseline_occurrence_after=$(wait_for_occurrence_increase \
  "$baseline_event_id" "$baseline_occurrence")
inode_before=$(sudo stat -c '%i' "$database")

sudo docker pause "$fault_container" >/dev/null
paused=true
degraded=$(wait_for_aggregate DEGRADED 2)
degraded_alerts=$(alerts)
assert_no_broadcast <<<"$degraded_alerts"
degraded_event_id=$(jq -r '.events[0].event_id' <<<"$degraded_alerts")
test "$degraded_event_id" != "$baseline_event_id"
jq -e '
  .events[0].status == "DEGRADED" and
  .events[0].operator_attention_required == true and
  .events[0].resolved_at == null
' <<<"$degraded_alerts" >/dev/null

acknowledgement_body=$(jq -nc \
  --arg protocol_version "$protocol_version" \
  '{protocol_version:$protocol_version,operator_id:"vps-canary",note:"controlled read-only failover verified"}')
acknowledgement_url="$aggregate_url/api/v3.6/alerts/$degraded_event_id/acknowledge"

missing_token_status=$(curl --silent --output "$response_file" --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --data "$acknowledgement_body" \
  "$acknowledgement_url")
test "$missing_token_status" = 401
assert_no_broadcast <"$response_file"

wrong_token_status=$(curl --silent --output "$response_file" --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --header 'Authorization: Bearer deliberately-wrong-token' \
  --data "$acknowledgement_body" \
  "$acknowledgement_url")
test "$wrong_token_status" = 401
assert_no_broadcast <"$response_file"

token=$(sudo cat "$token_file")
wrong_protocol_body=$(jq -nc \
  '{protocol_version:"0x9999",operator_id:"vps-canary",note:"controlled read-only failover verified"}')
wrong_protocol_status=$(curl --silent --output "$response_file" --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --header "Authorization: Bearer $token" \
  --data "$wrong_protocol_body" \
  "$acknowledgement_url")
test "$wrong_protocol_status" = 400
assert_no_broadcast <"$response_file"

acknowledged=$(curl --fail --silent \
  --header 'content-type: application/json' \
  --header "Authorization: Bearer $token" \
  --data "$acknowledgement_body" \
  "$acknowledgement_url")
assert_no_broadcast <<<"$acknowledged"
jq -e '
  .status == "ACKNOWLEDGED" and
  .event.acknowledged_by == "vps-canary" and
  .event.resolved_at == null
' <<<"$acknowledged" >/dev/null
if sudo docker logs "$aggregate_container" 2>&1 | grep -Fq "$token"; then
  echo "Bearer token appeared in aggregate container logs" >&2
  exit 1
fi
unset token

sudo docker unpause "$fault_container" >/dev/null
paused=false
recovered=$(wait_for_aggregate READY 3)
recovered_alerts=$(alerts)
assert_no_broadcast <<<"$recovered_alerts"
recovery_event_id=$(jq -r '.events[0].event_id' <<<"$recovered_alerts")
test "$recovery_event_id" != "$degraded_event_id"
jq -e --argjson degraded_event_id "$degraded_event_id" '
  any(.events[];
    .event_id == $degraded_event_id and
    .acknowledged_by == "vps-canary" and
    .resolved_at != null)
' <<<"$recovered_alerts" >/dev/null

event_count_before_recreate=$(jq -r '.events | length' <<<"$recovered_alerts")
sudo docker compose -f "$overlay" up -d --force-recreate --no-deps monitor-aggregate >/dev/null
wait_for_healthy "$aggregate_container"
post_recreate_alerts=$(alerts)
assert_no_broadcast <<<"$post_recreate_alerts"
inode_after=$(sudo stat -c '%i' "$database")
test "$inode_after" = "$inode_before"
jq -e --argjson degraded_event_id "$degraded_event_id" '
  any(.events[];
    .event_id == $degraded_event_id and
    .acknowledged_by == "vps-canary" and
    .resolved_at != null)
' <<<"$post_recreate_alerts" >/dev/null

containers=$(for container in \
  xhub-v36-monitor-a \
  xhub-v36-monitor-b \
  xhub-v36-monitor-c \
  "$aggregate_container"; do
  sudo docker inspect "$container" | jq '.[0] | {
    name:(.Name | ltrimstr("/")),
    health:.State.Health.Status,
    restart_count:.RestartCount,
    image_id:.Image
  }'
done | jq -s '.')
jq -e 'all(.[]; .health == "healthy" and .restart_count == 0)' \
  <<<"$containers" >/dev/null

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson containers "$containers" \
  --argjson baseline "$baseline" \
  --argjson degraded "$degraded" \
  --argjson recovered "$recovered" \
  --argjson baseline_event_id "$baseline_event_id" \
  --argjson baseline_occurrence "$baseline_occurrence" \
  --argjson baseline_occurrence_after "$baseline_occurrence_after" \
  --arg baseline_fingerprint "$baseline_fingerprint" \
  --argjson degraded_event_id "$degraded_event_id" \
  --argjson recovery_event_id "$recovery_event_id" \
  --argjson event_count_before_recreate "$event_count_before_recreate" \
  --arg inode_before "$inode_before" \
  --arg inode_after "$inode_after" \
  --arg missing_token_status "$missing_token_status" \
  --arg wrong_token_status "$wrong_token_status" \
  --arg wrong_protocol_status "$wrong_protocol_status" \
  '{
    schema:"xhub-v3-6-readonly-monitor-alert-lifecycle-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    deployment:{physical_failure_domain_count:1,production_ready:false},
    containers:$containers,
    baseline:{
      status:$baseline.status,
      agreeing_count:$baseline.agreeing_count,
      event_id:$baseline_event_id,
      fingerprint:$baseline_fingerprint,
      occurrence_before:$baseline_occurrence,
      occurrence_after:$baseline_occurrence_after
    },
    controlled_degradation:{
      status:$degraded.status,
      agreeing_count:$degraded.agreeing_count,
      event_id:$degraded_event_id,
      missing_token_http_status:($missing_token_status | tonumber),
      wrong_token_http_status:($wrong_token_status | tonumber),
      wrong_protocol_http_status:($wrong_protocol_status | tonumber),
      acknowledgement_persisted:true,
      resolved_after_recovery:true
    },
    recovery:{
      status:$recovered.status,
      agreeing_count:$recovered.agreeing_count,
      event_id:$recovery_event_id
    },
    persistence:{
      event_count_before_recreate:$event_count_before_recreate,
      sqlite_inode_before:$inode_before,
      sqlite_inode_after:$inode_after,
      acknowledged_event_present_after_recreate:true
    },
    token_present_in_logs:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
