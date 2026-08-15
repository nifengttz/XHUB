# XHUB V3.6 Single-VPS Monitor Alert Operations

This runbook applies only to the temporary single-VPS Docker deployment. It does not establish an independent failure domain, production readiness, or transaction broadcast authority.

## Scheduled Backup

Install the hardened daily systemd timer from the VPS staging directory:

```bash
sudo bash /home/ubuntu/xhub-v36-alert-staging/install-monitor-aggregate-operations.sh \
  /home/ubuntu/xhub-v36-alert-staging
```

The timer runs daily at 03:15 UTC with up to 15 minutes of randomized delay. It uses SQLite Online Backup, requires at least 1 GiB of free space, runs `PRAGMA quick_check`, and writes a SHA-256 manifest. It does not stop the monitor aggregator.

```bash
systemctl status xhub-v36-monitor-alert-backup.timer
systemctl list-timers xhub-v36-monitor-alert-backup.timer
sudo systemctl start xhub-v36-monitor-alert-backup.service
```

## Inspection

Run the local inspection after deployment changes, token rotation, backup failures, or host maintenance:

```bash
/opt/xhub-v3.6-test/operations/inspect-monitor-aggregate-operations.sh | jq .
```

The inspection fails closed when monitor quorum is unavailable, a container is unhealthy, the latest backup is older than 48 hours, the backup hash differs, free space is below 1 GiB, or the timer is not enabled and active.

The installed inspection timer runs every 15 minutes with up to two minutes of randomized delay:

```bash
systemctl status xhub-v36-monitor-operations-inspection.timer
sudo systemctl start xhub-v36-monitor-operations-inspection.service
sudo cat /opt/xhub-v3.6-test/operations-reports/monitor-aggregate/latest.json | jq .
```

Successful runs atomically replace `latest.json`. Failed runs leave the previous successful report unchanged, write `latest-failure.json`, and return a failed systemd service result. The task does not restart containers, acknowledge alerts, send external notifications, or repair state.

Every inspection also verifies the SHA-256 integrity manifest for the installed operations scripts and systemd units:

```bash
/opt/xhub-v3.6-test/operations/verify-monitor-aggregate-operations-integrity.sh | jq .
```

A missing file, unmanaged path, duplicate manifest entry, or hash mismatch fails the inspection. The verifier reports drift but never replaces the modified file. The local manifest is not an external trust anchor.

## Retention Candidates

The default policy keeps the latest seven valid backups and only lists older backups after seven days:

```bash
/opt/xhub-v3.6-test/operations/list-monitor-aggregate-backup-retention-candidates.sh | jq .
```

The command never deletes files. Every candidate requires manual review. Quarantined backups are excluded from valid backup enumeration.

## Restore Drill

Run a restore drill against a selected valid backup directory:

```bash
sudo bash /opt/xhub-v3.6-test/operations/verify-monitor-aggregate-alert-backup.sh \
  /opt/xhub-v3.6-test/backups/monitor-aggregate-alerts/alerts-YYYYMMDDTHHMMSSZ
```

The drill verifies the manifest and SHA-256, starts an isolated temporary aggregator on `127.0.0.1:18745`, compares every expected event ID, checks that all broadcast flags remain false, and removes the temporary container and database copy.

The installed weekly timer selects the latest valid backup and runs the same drill every Sunday at 04:00 UTC with up to 30 minutes of randomized delay:

```bash
systemctl status xhub-v36-monitor-alert-restore-drill.timer
sudo systemctl start xhub-v36-monitor-alert-restore-drill.service
sudo cat /opt/xhub-v3.6-test/operations-reports/monitor-aggregate-restore/latest.json | jq .
```

The scheduled drill never selects quarantined backups. A failed drill preserves the last successful report, writes `latest-failure.json`, and leaves source data unchanged.

## Token Rotation

Rotate the acknowledgement token without printing it:

```bash
sudo bash /opt/xhub-v3.6-test/operations/rotate-monitor-aggregate-token.sh
```

The rotation rolls back on failure, verifies that the old token returns `401`, and verifies that the new token passes authentication. Rotation does not acknowledge an event or submit a transaction.

## Fixed Boundaries

- Monitor APIs remain bound to loopback addresses.
- Backup manifests never contain API tokens.
- Automatic backup deletion is disabled.
- Email and external alert delivery are not configured.
- Automatic remediation is disabled.
- Operations file drift is detected but not automatically repaired.
- `physical_failure_domain_count=1` and `production_ready=false` remain fixed.
- `spend_bundle_created`, `broadcast_enabled`, `broadcast_ready`, and `chain_broadcast` remain false.
