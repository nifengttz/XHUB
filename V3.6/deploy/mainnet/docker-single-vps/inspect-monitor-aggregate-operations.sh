#!/usr/bin/env bash
set -euo pipefail

if (( EUID == 0 )); then
  sudo() { command "$@"; }
fi

root=/opt/xhub-v3.6-test
endpoint=http://127.0.0.1:18744
database="$root/data/monitor-aggregate/alerts.sqlite3"
backup_root="$root/backups/monitor-aggregate-alerts"
script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
retention_script="$script_directory/list-monitor-aggregate-backup-retention-candidates.sh"
integrity_verifier="$script_directory/verify-monitor-aggregate-operations-integrity.sh"
minimum_free_bytes=${XHUB_MONITOR_ALERT_BACKUP_MIN_FREE_BYTES:-1073741824}
maximum_backup_age_seconds=${XHUB_MONITOR_ALERT_MAX_BACKUP_AGE_SECONDS:-172800}
timer=xhub-v36-monitor-alert-backup.timer

command -v curl >/dev/null
command -v jq >/dev/null
command -v sha256sum >/dev/null
[[ "$minimum_free_bytes" =~ ^[1-9][0-9]*$ ]]
[[ "$maximum_backup_age_seconds" =~ ^[1-9][0-9]*$ ]]
test -x "$retention_script"
test -x "$integrity_verifier"
sudo test -s "$database"
sudo test -d "$backup_root"

aggregate=$(curl --fail --silent \
  "$endpoint/api/v3.6/monitor-aggregate?protocol_version=0x0360")
alerts=$(curl --fail --silent \
  "$endpoint/api/v3.6/alerts?protocol_version=0x0360&limit=500")
for body in "$aggregate" "$alerts"; do
  jq -e '
    .spend_bundle_created == false and
    .broadcast_enabled == false and
    .broadcast_ready == false and
    .chain_broadcast == false
  ' <<<"$body" >/dev/null
done

containers=$(for container in \
  xhub-v36-monitor-a \
  xhub-v36-monitor-b \
  xhub-v36-monitor-c \
  xhub-v36-monitor-aggregate; do
  sudo docker inspect "$container" | jq '.[0] | {
    name:(.Name | ltrimstr("/")),
    running:.State.Running,
    health:.State.Health.Status,
    restart_count:.RestartCount,
    image_id:.Image
  }'
done | jq -s '.')
jq -e 'all(.[]; .running == true and .health == "healthy")' \
  <<<"$containers" >/dev/null

latest_backup=$(sudo find "$backup_root" -mindepth 1 -maxdepth 1 -type d \
  -name 'alerts-*' -print | sort | tail -n 1)
test -n "$latest_backup"
[[ "$latest_backup" == "$backup_root"/alerts-* ]]
manifest=$(sudo cat "$latest_backup/manifest.json")
jq -e '
  .sqlite_online_backup == true and
  .quick_check == "ok" and
  .disk_capacity_gate_passed == true and
  .contains_api_token == false and
  .chain_broadcast == false
' <<<"$manifest" >/dev/null
expected_sha256=$(jq -r '.database_sha256' <<<"$manifest")
actual_sha256=$(sudo sha256sum "$latest_backup/alerts.sqlite3" | awk '{print $1}')
test "$actual_sha256" = "$expected_sha256"
created_epoch=$(date -u -d "$(jq -r '.created_at' <<<"$manifest")" +%s)
now=$(date -u +%s)
backup_age_seconds=$((now - created_epoch))
(( backup_age_seconds >= 0 ))
(( backup_age_seconds <= maximum_backup_age_seconds ))

available_bytes=$(sudo df --output=avail -B1 "$backup_root" | tail -n 1 | tr -d ' ')
[[ "$available_bytes" =~ ^[0-9]+$ ]]
(( available_bytes >= minimum_free_bytes ))
timer_enabled=$(systemctl is-enabled "$timer")
timer_active=$(systemctl is-active "$timer")
test "$timer_enabled" = enabled
test "$timer_active" = active
next_run=$(systemctl show "$timer" --property NextElapseUSecRealtime --value)
test -n "$next_run"
retention=$("$retention_script")
jq -e '.files_deleted == false and .automatic_deletion_enabled == false' \
  <<<"$retention" >/dev/null
integrity=$("$integrity_verifier")
jq -e '.status == "PASS" and .all_files_match == true and .automatic_repair_enabled == false' \
  <<<"$integrity" >/dev/null

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson aggregate "$aggregate" \
  --argjson containers "$containers" \
  --argjson event_count "$(jq -r '.events | length' <<<"$alerts")" \
  --arg latest_backup_id "$(jq -r '.backup_id' <<<"$manifest")" \
  --arg latest_backup_sha256 "$actual_sha256" \
  --argjson backup_age_seconds "$backup_age_seconds" \
  --argjson maximum_backup_age_seconds "$maximum_backup_age_seconds" \
  --argjson available_bytes "$available_bytes" \
  --argjson minimum_free_bytes "$minimum_free_bytes" \
  --arg timer_enabled "$timer_enabled" \
  --arg timer_active "$timer_active" \
  --arg next_run "$next_run" \
  --argjson retention "$retention" \
  --argjson integrity "$integrity" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-operations-inspection-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    status:"PASS",
    aggregate:{status:$aggregate.status,agreeing_count:$aggregate.agreeing_count,quorum_reached:$aggregate.quorum_reached},
    containers:$containers,
    event_count:$event_count,
    backup:{
      backup_id:$latest_backup_id,
      database_sha256:$latest_backup_sha256,
      age_seconds:$backup_age_seconds,
      maximum_age_seconds:$maximum_backup_age_seconds,
      fresh:true
    },
    disk:{
      available_bytes:$available_bytes,
      minimum_free_bytes:$minimum_free_bytes,
      capacity_gate_passed:true
    },
    timer:{enabled:$timer_enabled,active:$timer_active,next_run:$next_run},
    retention:{
      valid_backup_count:$retention.valid_backup_count,
      candidate_count:$retention.candidate_count,
      files_deleted:false,
      manual_review_required:true
    },
    integrity:{
      status:$integrity.status,
      manifest_sha256:$integrity.manifest_sha256,
      verified_file_count:$integrity.verified_file_count,
      all_files_match:$integrity.all_files_match,
      automatic_repair_enabled:false
    },
    physical_failure_domain_count:1,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
