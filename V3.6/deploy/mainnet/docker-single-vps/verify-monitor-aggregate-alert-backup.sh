#!/usr/bin/env bash
set -euo pipefail

if (( EUID == 0 )); then
  sudo() { command "$@"; }
fi

backup_directory=${1:?usage: verify-monitor-aggregate-alert-backup.sh BACKUP_DIRECTORY}
root=/opt/xhub-v3.6-test
backup_root="$root/backups/monitor-aggregate-alerts"
token_file="$root/secrets/monitor-aggregate-api-token.txt"
image=xhub-watchtower-v3-6:readonly-monitor
funding_coin_id=08a7d9ead0fb2aafa79f282e0378d0811b6b7dba948667cfdd9ada4933de0989
listen=127.0.0.1:18745
endpoint=http://127.0.0.1:18745
drill_id="restore-drill-$(date -u +%Y%m%dT%H%M%SZ)-$$"
drill_directory="$root/restore-drills/$drill_id"
drill_container="xhub-v36-monitor-aggregate-$drill_id"
container_started=false

cleanup() {
  set +e
  if [[ "$container_started" == true ]]; then
    sudo docker rm --force "$drill_container" >/dev/null 2>&1
  fi
  if [[ "$drill_directory" == "$root"/restore-drills/restore-drill-* ]]; then
    sudo rm -rf "$drill_directory"
  fi
}
trap cleanup EXIT

command -v curl >/dev/null
command -v jq >/dev/null
command -v sha256sum >/dev/null
sudo test -s "$token_file"
[[ "$backup_directory" == "$backup_root"/alerts-* ]]
sudo test -s "$backup_directory/manifest.json"
sudo test -s "$backup_directory/alerts.sqlite3"
[[ "$drill_directory" == "$root"/restore-drills/restore-drill-* ]]
manifest=$(sudo cat "$backup_directory/manifest.json")
jq -e '
  .schema == "xhub-v3-6-monitor-aggregate-alert-backup-1" and
  .protocol_version == "0x0360" and
  .sqlite_online_backup == true and
  .quick_check == "ok" and
  .consistent_shutdown == false and
  .contains_api_token == false and
  .production_ready == false and
  .spend_bundle_created == false and
  .broadcast_enabled == false and
  .broadcast_ready == false and
  .chain_broadcast == false
' <<<"$manifest" >/dev/null
expected_sha256=$(jq -r '.database_sha256' <<<"$manifest")
actual_sha256=$(sudo sha256sum "$backup_directory/alerts.sqlite3" | awk '{print $1}')
test "$actual_sha256" = "$expected_sha256"
expected_event_ids=$(jq -c '.event_ids | sort' <<<"$manifest")
expected_event_count=$(jq -r '.event_count' <<<"$manifest")

sudo install -d -o 65532 -g 65532 -m 0750 "$drill_directory"
sudo cp "$backup_directory/alerts.sqlite3" "$drill_directory/alerts.sqlite3"
sudo chown 65532:65532 "$drill_directory/alerts.sqlite3"
sudo chmod 0600 "$drill_directory/alerts.sqlite3"
sudo docker run --detach \
  --name "$drill_container" \
  --network host \
  --user 65532:65532 \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --tmpfs /tmp:rw,noexec,nosuid,size=8m \
  --volume "$drill_directory:/var/lib/xhub-monitor-aggregate" \
  --volume "$token_file:/run/secrets/monitor-aggregate-api-token.txt:ro" \
  --entrypoint /usr/local/bin/watchtower-monitor-aggregate-v3-6 \
  "$image" \
  /var/lib/xhub-monitor-aggregate/alerts.sqlite3 \
  /run/secrets/monitor-aggregate-api-token.txt \
  "$funding_coin_id" \
  300 \
  "$listen" \
  http://127.0.0.1:18741 \
  http://127.0.0.1:18742 \
  http://127.0.0.1:18743 >/dev/null
container_started=true

alerts=
for _ in $(seq 1 30); do
  alerts=$(curl --silent \
    "$endpoint/api/v3.6/alerts?protocol_version=0x0360&limit=500" || true)
  if jq -e '.events | length > 0' <<<"$alerts" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
jq -e '
  (.events | length) > 0 and
  .spend_bundle_created == false and
  .broadcast_enabled == false and
  .broadcast_ready == false and
  .chain_broadcast == false
' <<<"$alerts" >/dev/null
restored_event_ids=$(jq -c '[.events[].event_id] | sort' <<<"$alerts")
test "$restored_event_ids" = "$expected_event_ids"
restored_event_count=$(jq -r '.events | length' <<<"$alerts")
test "$restored_event_count" = "$expected_event_count"
token=$(sudo cat "$token_file")
if sudo docker logs "$drill_container" 2>&1 | grep -Fq "$token"; then
  echo "Bearer token appeared in restore drill logs" >&2
  exit 1
fi
unset token

container_image_id=$(sudo docker inspect --format '{{.Image}}' "$drill_container")
sudo docker rm --force "$drill_container" >/dev/null
container_started=false
sudo rm -rf "$drill_directory"
trap - EXIT

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg backup_id "$(jq -r '.backup_id' <<<"$manifest")" \
  --arg database_sha256 "$actual_sha256" \
  --argjson restored_event_count "$restored_event_count" \
  --arg container_image_id "$container_image_id" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-alert-restore-drill-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    backup_id:$backup_id,
    database_sha256:$database_sha256,
    restored_event_count:$restored_event_count,
    all_expected_event_ids_present:true,
    isolated_listen:"127.0.0.1:18745",
    drill_container_removed:true,
    source_unchanged:true,
    token_present_in_logs:false,
    container_image_id:$container_image_id,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
