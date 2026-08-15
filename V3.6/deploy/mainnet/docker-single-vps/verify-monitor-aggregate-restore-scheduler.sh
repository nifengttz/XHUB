#!/usr/bin/env bash
set -euo pipefail

if (( EUID == 0 )); then
  sudo() { command "$@"; }
fi

root=/opt/xhub-v3.6-test
report="$root/operations-reports/monitor-aggregate-restore/latest.json"
failure="$root/operations-reports/monitor-aggregate-restore/latest-failure.json"
service=xhub-v36-monitor-alert-restore-drill.service
timer=xhub-v36-monitor-alert-restore-drill.timer
maximum_report_age_seconds=${XHUB_MONITOR_RESTORE_DRILL_MAX_AGE_SECONDS:-691200}

command -v jq >/dev/null
command -v sha256sum >/dev/null
[[ "$maximum_report_age_seconds" =~ ^[1-9][0-9]*$ ]]
sudo test -s "$report"
sudo test ! -e "$failure"
restore=$(sudo cat "$report")
jq -e '
  .all_expected_event_ids_present == true and
  .drill_container_removed == true and
  .source_unchanged == true and
  .token_present_in_logs == false and
  .production_ready == false and
  .spend_bundle_created == false and
  .broadcast_enabled == false and
  .broadcast_ready == false and
  .chain_broadcast == false
' <<<"$restore" >/dev/null
observed_epoch=$(date -u -d "$(jq -r '.observed_at' <<<"$restore")" +%s)
now=$(date -u +%s)
report_age_seconds=$((now - observed_epoch))
(( report_age_seconds >= 0 ))
(( report_age_seconds <= maximum_report_age_seconds ))
service_result=$(systemctl show "$service" --property Result --value)
timer_enabled=$(systemctl is-enabled "$timer")
timer_active=$(systemctl is-active "$timer")
test "$service_result" = success
test "$timer_enabled" = enabled
test "$timer_active" = active
next_run=$(systemctl show "$timer" --property NextElapseUSecRealtime --value)
test -n "$next_run"
report_sha256=$(sudo sha256sum "$report" | awk '{print $1}')

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg backup_id "$(jq -r '.backup_id' <<<"$restore")" \
  --arg report_sha256 "$report_sha256" \
  --argjson report_age_seconds "$report_age_seconds" \
  --argjson maximum_report_age_seconds "$maximum_report_age_seconds" \
  --arg service_result "$service_result" \
  --arg timer_enabled "$timer_enabled" \
  --arg timer_active "$timer_active" \
  --arg next_run "$next_run" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-restore-scheduler-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    backup_id:$backup_id,
    report_sha256:$report_sha256,
    report_age_seconds:$report_age_seconds,
    maximum_report_age_seconds:$maximum_report_age_seconds,
    report_fresh:true,
    report_atomic_publish:true,
    failure_marker_present:false,
    service_result:$service_result,
    timer:{enabled:$timer_enabled,active:$timer_active,next_run:$next_run},
    automatic_remediation_enabled:false,
    external_notification_enabled:false,
    physical_failure_domain_count:1,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
