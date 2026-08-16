#!/usr/bin/env bash
set -euo pipefail

if (( EUID == 0 )); then
  sudo() { command "$@"; }
fi

root=/opt/xhub-v3.6-test
backup_root="$root/backups/monitor-aggregate-alerts"
keep_latest=${1:-7}
minimum_age_seconds=${2:-604800}
now=$(date -u +%s)

command -v jq >/dev/null
command -v sha256sum >/dev/null
[[ "$keep_latest" =~ ^[1-9][0-9]*$ ]]
[[ "$minimum_age_seconds" =~ ^[0-9]+$ ]]
sudo test -d "$backup_root"

records=$(
  while IFS= read -r directory; do
    [[ "$directory" == "$backup_root"/alerts-* ]]
    manifest_path="$directory/manifest.json"
    database_path="$directory/alerts.sqlite3"
    sudo test -s "$manifest_path"
    sudo test -s "$database_path"
    manifest=$(sudo cat "$manifest_path")
    jq -e '
      .schema == "xhub-v3-6-monitor-aggregate-alert-backup-1" and
      .protocol_version == "0x0360" and
      .sqlite_online_backup == true and
      .quick_check == "ok" and
      .contains_api_token == false and
      .production_ready == false and
      .chain_broadcast == false
    ' <<<"$manifest" >/dev/null
    expected_sha256=$(jq -r '.database_sha256' <<<"$manifest")
    actual_sha256=$(sudo sha256sum "$database_path" | awk '{print $1}')
    test "$actual_sha256" = "$expected_sha256"
    created_at=$(jq -r '.created_at' <<<"$manifest")
    created_epoch=$(date -u -d "$created_at" +%s)
    age_seconds=$((now - created_epoch))
    (( age_seconds >= 0 ))
    jq -nc \
      --arg backup_id "$(jq -r '.backup_id' <<<"$manifest")" \
      --arg created_at "$created_at" \
      --arg database_sha256 "$actual_sha256" \
      --argjson created_epoch "$created_epoch" \
      --argjson age_seconds "$age_seconds" \
      '{
        backup_id:$backup_id,
        created_at:$created_at,
        created_epoch:$created_epoch,
        age_seconds:$age_seconds,
        database_sha256:$database_sha256
      }'
  done < <(sudo find "$backup_root" -mindepth 1 -maxdepth 1 -type d -name 'alerts-*' -print | sort)
)
all_backups=$(jq -s 'sort_by(.created_epoch) | reverse' <<<"$records")
candidates=$(jq \
  --argjson keep_latest "$keep_latest" \
  --argjson minimum_age_seconds "$minimum_age_seconds" \
  'to_entries |
   map(select(.key >= $keep_latest and .value.age_seconds >= $minimum_age_seconds)) |
   map(.value)' <<<"$all_backups")

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson keep_latest "$keep_latest" \
  --argjson minimum_age_seconds "$minimum_age_seconds" \
  --argjson all_backups "$all_backups" \
  --argjson candidates "$candidates" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-alert-retention-candidates-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    keep_latest:$keep_latest,
    minimum_age_seconds:$minimum_age_seconds,
    valid_backup_count:($all_backups | length),
    candidate_count:($candidates | length),
    candidates:$candidates,
    files_deleted:false,
    automatic_deletion_enabled:false,
    manual_review_required:true,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
