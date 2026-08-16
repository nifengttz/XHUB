#!/usr/bin/env bash
set -euo pipefail

if (( EUID == 0 )); then
  sudo() { command "$@"; }
fi

root=/opt/xhub-v3.6-test
container=xhub-v36-monitor-aggregate
endpoint=http://127.0.0.1:18744
database="$root/data/monitor-aggregate/alerts.sqlite3"
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
online_backup="$script_directory/sqlite-online-backup.py"
backup_root="$root/backups/monitor-aggregate-alerts"
minimum_free_bytes=${XHUB_MONITOR_ALERT_BACKUP_MIN_FREE_BYTES:-1073741824}
backup_id="alerts-$(date -u +%Y%m%dT%H%M%SZ)"
staging="$backup_root/.${backup_id}.tmp"
destination="$backup_root/$backup_id"

cleanup() {
  local exit_code=$?
  set +e
  if [[ "$staging" == "$backup_root"/.alerts-*.tmp ]]; then
    sudo rm -rf "$staging"
  fi
  exit "$exit_code"
}
trap cleanup ERR INT TERM

command -v curl >/dev/null
command -v jq >/dev/null
command -v python3 >/dev/null
command -v sha256sum >/dev/null
test -f "$online_backup"
[[ "$minimum_free_bytes" =~ ^[1-9][0-9]*$ ]]
sudo test -s "$database"
sudo test ! -e "$destination"
[[ "$staging" == "$backup_root"/.alerts-*.tmp ]]
[[ "$destination" == "$backup_root"/alerts-* ]]
sudo install -d -o root -g root -m 0700 "$backup_root"
available_bytes_before=$(sudo df --output=avail -B1 "$backup_root" | tail -n 1 | tr -d ' ')
[[ "$available_bytes_before" =~ ^[0-9]+$ ]]
(( available_bytes_before >= minimum_free_bytes ))
alerts=$(curl --fail --silent \
  "$endpoint/api/v3.6/alerts?protocol_version=0x0360&limit=500")
jq -e '
  .spend_bundle_created == false and
  .broadcast_enabled == false and
  .broadcast_ready == false and
  .chain_broadcast == false
' <<<"$alerts" >/dev/null
event_ids=$(jq -c '[.events[].event_id]' <<<"$alerts")
event_count=$(jq -r '.events | length' <<<"$alerts")
latest_event_id=$(jq -r '.events[0].event_id' <<<"$alerts")
image_id=$(sudo docker inspect --format '{{.Image}}' "$container")

sudo install -d -o root -g root -m 0700 "$staging"
sudo python3 "$online_backup" "$database" "$staging/alerts.sqlite3" | \
  sudo tee "$staging/online-backup-report.json" >/dev/null
jq -e \
  --argjson event_count "$event_count" \
  '.quick_check == "ok" and .event_count == $event_count' \
  < <(sudo cat "$staging/online-backup-report.json") >/dev/null
sudo chown root:root "$staging/alerts.sqlite3"
sudo chmod 0600 "$staging/alerts.sqlite3"
sudo chown root:root "$staging/online-backup-report.json"
sudo chmod 0600 "$staging/online-backup-report.json"
database_sha256=$(sudo sha256sum "$staging/alerts.sqlite3" | awk '{print $1}')
database_size=$(sudo stat -c '%s' "$staging/alerts.sqlite3")
created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq -n \
  --arg backup_id "$backup_id" \
  --arg created_at "$created_at" \
  --arg database_sha256 "$database_sha256" \
  --argjson database_size "$database_size" \
  --argjson event_ids "$event_ids" \
  --argjson event_count "$event_count" \
  --argjson latest_event_id "$latest_event_id" \
  --argjson minimum_free_bytes "$minimum_free_bytes" \
  --argjson available_bytes_before "$available_bytes_before" \
  --arg image_id "$image_id" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-alert-backup-1",
    protocol_version:"0x0360",
    backup_id:$backup_id,
    created_at:$created_at,
    database_file:"alerts.sqlite3",
    database_sha256:$database_sha256,
    database_size:$database_size,
    event_ids:$event_ids,
    event_count:$event_count,
    latest_event_id:$latest_event_id,
    minimum_free_bytes:$minimum_free_bytes,
    available_bytes_before:$available_bytes_before,
    disk_capacity_gate_passed:true,
    source_image_id:$image_id,
    sqlite_online_backup:true,
    quick_check:"ok",
    consistent_shutdown:false,
    contains_api_token:false,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }' | sudo tee "$staging/manifest.json" >/dev/null
sudo chmod 0600 "$staging/manifest.json"
sudo mv "$staging" "$destination"

sudo test "$(sudo sha256sum "$destination/alerts.sqlite3" | awk '{print $1}')" = \
  "$database_sha256"

trap - ERR INT TERM
sudo cat "$destination/manifest.json"
