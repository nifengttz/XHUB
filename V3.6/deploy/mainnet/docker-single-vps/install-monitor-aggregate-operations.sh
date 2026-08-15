#!/usr/bin/env bash
set -euo pipefail

root=/opt/xhub-v3.6-test
staging=${1:-/home/ubuntu/xhub-v36-alert-staging}
operations="$root/operations"
backup_root="$root/backups/monitor-aggregate-alerts"
report_root="$root/operations-reports/monitor-aggregate"
restore_report_root="$root/operations-reports/monitor-aggregate-restore"
restore_drill_root="$root/restore-drills"
backup_service=xhub-v36-monitor-alert-backup.service
backup_timer=xhub-v36-monitor-alert-backup.timer
inspection_service=xhub-v36-monitor-operations-inspection.service
inspection_timer=xhub-v36-monitor-operations-inspection.timer
restore_service=xhub-v36-monitor-alert-restore-drill.service
restore_timer=xhub-v36-monitor-alert-restore-drill.timer
integrity_manifest=operations-integrity-manifest.json

for file in \
  sqlite-online-backup.py \
  backup-monitor-aggregate-alerts.sh \
  rotate-monitor-aggregate-token.sh \
  verify-monitor-aggregate-alert-backup.sh \
  list-monitor-aggregate-backup-retention-candidates.sh \
  inspect-monitor-aggregate-operations.sh \
  publish-monitor-aggregate-operations-inspection.sh \
  verify-monitor-aggregate-inspection-scheduler.sh \
  publish-latest-monitor-aggregate-restore-drill.sh \
  verify-monitor-aggregate-restore-scheduler.sh \
  verify-monitor-aggregate-operations-integrity.sh \
  "$integrity_manifest" \
  "$backup_service" \
  "$backup_timer" \
  "$inspection_service" \
  "$inspection_timer" \
  "$restore_service" \
  "$restore_timer"; do
  test -f "$staging/$file"
done

manifest=$(cat "$staging/$integrity_manifest")
jq -e '
  .schema == "xhub-v3-6-monitor-aggregate-operations-integrity-1" and
  .protocol_version == "0x0360" and
  .production_ready == false and
  .chain_broadcast == false
' <<<"$manifest" >/dev/null
while IFS=$'\t' read -r source_file expected_sha256; do
  test -f "$staging/$source_file"
  test "$(sha256sum "$staging/$source_file" | awk '{print $1}')" = "$expected_sha256"
done < <(jq -r '.files[] | [.source_file,.sha256] | @tsv' <<<"$manifest")

install -d -o root -g root -m 0755 "$operations"
install -d -o root -g root -m 0700 "$backup_root"
install -d -o root -g root -m 0700 "$report_root"
install -d -o root -g root -m 0700 "$restore_report_root" "$restore_drill_root"
install -o root -g root -m 0755 \
  "$staging/sqlite-online-backup.py" \
  "$staging/backup-monitor-aggregate-alerts.sh" \
  "$staging/rotate-monitor-aggregate-token.sh" \
  "$staging/verify-monitor-aggregate-alert-backup.sh" \
  "$staging/list-monitor-aggregate-backup-retention-candidates.sh" \
  "$staging/inspect-monitor-aggregate-operations.sh" \
  "$staging/publish-monitor-aggregate-operations-inspection.sh" \
  "$staging/verify-monitor-aggregate-inspection-scheduler.sh" \
  "$staging/publish-latest-monitor-aggregate-restore-drill.sh" \
  "$staging/verify-monitor-aggregate-restore-scheduler.sh" \
  "$staging/verify-monitor-aggregate-operations-integrity.sh" \
  "$operations/"
install -o root -g root -m 0644 \
  "$staging/$integrity_manifest" \
  "$operations/$integrity_manifest"
for unit in \
  "$backup_service" \
  "$backup_timer" \
  "$inspection_service" \
  "$inspection_timer" \
  "$restore_service" \
  "$restore_timer"; do
  install -o root -g root -m 0644 "$staging/$unit" "/etc/systemd/system/$unit"
done

systemd-analyze verify \
  "/etc/systemd/system/$backup_service" \
  "/etc/systemd/system/$backup_timer" \
  "/etc/systemd/system/$inspection_service" \
  "/etc/systemd/system/$inspection_timer" \
  "/etc/systemd/system/$restore_service" \
  "/etc/systemd/system/$restore_timer"
systemctl daemon-reload
"$operations/verify-monitor-aggregate-operations-integrity.sh" >/dev/null
systemctl enable --now "$backup_timer" "$inspection_timer" "$restore_timer" >/dev/null
systemctl start "$backup_service"
systemctl start "$inspection_service"
systemctl start "$restore_service"
test "$(systemctl show "$backup_service" --property Result --value)" = success
test "$(systemctl show "$inspection_service" --property Result --value)" = success
test "$(systemctl show "$restore_service" --property Result --value)" = success
for timer in "$backup_timer" "$inspection_timer" "$restore_timer"; do
  test "$(systemctl is-enabled "$timer")" = enabled
  test "$(systemctl is-active "$timer")" = active
done
"$operations/verify-monitor-aggregate-inspection-scheduler.sh" >/dev/null
"$operations/verify-monitor-aggregate-restore-scheduler.sh" >/dev/null

echo "status=MONITOR_AGGREGATE_OPERATIONS_INSTALLED"
echo "backup_service=$backup_service"
echo "backup_timer=$backup_timer"
echo "inspection_service=$inspection_service"
echo "inspection_timer=$inspection_timer"
echo "restore_service=$restore_service"
echo "restore_timer=$restore_timer"
echo "operations=$operations"
echo "automatic_backup_enabled=true"
echo "automatic_deletion_enabled=false"
echo "automatic_remediation_enabled=false"
echo "external_notification_enabled=false"
echo "production_ready=false"
echo "chain_broadcast=false"
