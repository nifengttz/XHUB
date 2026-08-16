#!/usr/bin/env bash
set -euo pipefail

if (( EUID != 0 )); then
  echo "restore drill publisher must run as root" >&2
  exit 1
fi

root=/opt/xhub-v3.6-test
backup_root="$root/backups/monitor-aggregate-alerts"
operations="$root/operations"
restore_drill="$operations/verify-monitor-aggregate-alert-backup.sh"
report_root="$root/operations-reports/monitor-aggregate-restore"
latest="$report_root/latest.json"
failure="$report_root/latest-failure.json"
resolved_failure="$report_root/last-resolved-failure.json"
temporary=
error_file=

cleanup() {
  local exit_code=$?
  set +e
  if [[ -n "$temporary" && "$temporary" == "$report_root"/.restore-* ]]; then
    rm -f "$temporary"
  fi
  if [[ -n "$error_file" && "$error_file" == "$report_root"/.restore-error-* ]]; then
    rm -f "$error_file"
  fi
  exit "$exit_code"
}
trap cleanup ERR INT TERM

command -v jq >/dev/null
command -v sha256sum >/dev/null
test -x "$restore_drill"
test -d "$backup_root"
latest_backup=$(find "$backup_root" -mindepth 1 -maxdepth 1 -type d \
  -name 'alerts-*' -print | sort | tail -n 1)
test -n "$latest_backup"
[[ "$latest_backup" == "$backup_root"/alerts-* ]]
[[ "$latest" == "$report_root/latest.json" ]]
install -d -o root -g root -m 0700 "$report_root"
temporary=$(mktemp "$report_root/.restore-XXXXXX")
error_file=$(mktemp "$report_root/.restore-error-XXXXXX")

if "$restore_drill" "$latest_backup" >"$temporary" 2>"$error_file"; then
  jq -e '
    .schema == "xhub-v3-6-monitor-aggregate-alert-restore-drill-evidence-1" and
    .all_expected_event_ids_present == true and
    .drill_container_removed == true and
    .source_unchanged == true and
    .token_present_in_logs == false and
    .production_ready == false and
    .spend_bundle_created == false and
    .broadcast_enabled == false and
    .broadcast_ready == false and
    .chain_broadcast == false
  ' "$temporary" >/dev/null
  chmod 0600 "$temporary"
  mv -f "$temporary" "$latest"
  temporary=
  if [[ -f "$failure" ]]; then
    mv -f "$failure" "$resolved_failure"
  fi
  rm -f "$error_file"
  error_file=
  report_sha256=$(sha256sum "$latest" | awk '{print $1}')
  jq -n \
    --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg backup_id "$(jq -r '.backup_id' "$latest")" \
    --arg report_sha256 "$report_sha256" \
    '{
      schema:"xhub-v3-6-monitor-aggregate-scheduled-restore-publication-1",
      protocol_version:"0x0360",
      observed_at:$observed_at,
      status:"PUBLISHED",
      backup_id:$backup_id,
      report_sha256:$report_sha256,
      failure_marker_present:false,
      automatic_remediation_enabled:false,
      external_notification_enabled:false,
      production_ready:false,
      spend_bundle_created:false,
      broadcast_enabled:false,
      broadcast_ready:false,
      chain_broadcast:false
    }'
else
  restore_exit_code=$?
  rm -f "$temporary" "$error_file"
  error_file=
  temporary=$(mktemp "$report_root/.restore-failure-XXXXXX")
  jq -n \
    --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson restore_exit_code "$restore_exit_code" \
    '{
      schema:"xhub-v3-6-monitor-aggregate-scheduled-restore-failure-1",
      protocol_version:"0x0360",
      observed_at:$observed_at,
      status:"FAIL",
      restore_exit_code:$restore_exit_code,
      error_detail_stored:false,
      operator_attention_required:true,
      automatic_remediation_enabled:false,
      external_notification_enabled:false,
      production_ready:false,
      spend_bundle_created:false,
      broadcast_enabled:false,
      broadcast_ready:false,
      chain_broadcast:false
    }' >"$temporary"
  chmod 0600 "$temporary"
  mv -f "$temporary" "$failure"
  temporary=
  exit "$restore_exit_code"
fi

trap - ERR INT TERM
