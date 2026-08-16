$ErrorActionPreference="Stop"
$validator=Join-Path $PSScriptRoot "validate-single-vps-docker-profile.ps1"
$generator=Join-Path $PSScriptRoot "generate-single-vps-docker-compose.ps1"
$dockerfile=Join-Path $PSScriptRoot "Dockerfile"
$readonlyOverlay=Join-Path $PSScriptRoot "compose.readonly-monitors.yaml"
$readonlyInstaller=Join-Path $PSScriptRoot "install-readonly-monitors.sh"
$failoverVerifier=Join-Path $PSScriptRoot "verify-aggregator-failover.sh"
$alertVerifier=Join-Path $PSScriptRoot "verify-alert-lifecycle.sh"
$tokenRotator=Join-Path $PSScriptRoot "rotate-monitor-aggregate-token.sh"
$alertBackup=Join-Path $PSScriptRoot "backup-monitor-aggregate-alerts.sh"
$alertRestoreDrill=Join-Path $PSScriptRoot "verify-monitor-aggregate-alert-backup.sh"
$sqliteOnlineBackup=Join-Path $PSScriptRoot "sqlite-online-backup.py"
$retentionCandidates=Join-Path $PSScriptRoot "list-monitor-aggregate-backup-retention-candidates.sh"
$operationsInspection=Join-Path $PSScriptRoot "inspect-monitor-aggregate-operations.sh"
$operationsInstaller=Join-Path $PSScriptRoot "install-monitor-aggregate-operations.sh"
$backupService=Join-Path $PSScriptRoot "xhub-v36-monitor-alert-backup.service"
$backupTimer=Join-Path $PSScriptRoot "xhub-v36-monitor-alert-backup.timer"
$inspectionPublisher=Join-Path $PSScriptRoot "publish-monitor-aggregate-operations-inspection.sh"
$inspectionSchedulerVerifier=Join-Path $PSScriptRoot "verify-monitor-aggregate-inspection-scheduler.sh"
$inspectionService=Join-Path $PSScriptRoot "xhub-v36-monitor-operations-inspection.service"
$inspectionTimer=Join-Path $PSScriptRoot "xhub-v36-monitor-operations-inspection.timer"
$restorePublisher=Join-Path $PSScriptRoot "publish-latest-monitor-aggregate-restore-drill.sh"
$restoreSchedulerVerifier=Join-Path $PSScriptRoot "verify-monitor-aggregate-restore-scheduler.sh"
$restoreService=Join-Path $PSScriptRoot "xhub-v36-monitor-alert-restore-drill.service"
$restoreTimer=Join-Path $PSScriptRoot "xhub-v36-monitor-alert-restore-drill.timer"
$integrityVerifier=Join-Path $PSScriptRoot "verify-monitor-aggregate-operations-integrity.sh"
$integrityManifest=Join-Path $PSScriptRoot "operations-integrity-manifest.json"
$integrityFailClosed=Join-Path $PSScriptRoot "verify-monitor-aggregate-operations-integrity-fail-closed.sh"
$dockerignore=Join-Path (Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent) ".dockerignore"
function New-Fixtures{
    $instances=@();$attesters=@()
    for($index=0;$index-lt3;$index++){$suffix=[char]([int][char]'a'+$index);$id="wt-$suffix";$instances += [ordered]@{attester_id=$id;listen_port=(18738+$index);database_directory="/opt/xhub-v3.6/data/$id";api_token_file="/opt/xhub-v3.6/secrets/$id-token.txt"};$attesters += [ordered]@{signer_id=$id;failure_domain="single-tencent-vps";signer_public_key=((@("{0:x2}"-f(0x81+$index))*48)-join"")}}
    [pscustomobject]@{Profile=[ordered]@{schema="xhub-v3-6-single-vps-docker-test-1";protocol_version="0x0360";mode="single-vps-docker-test";image="xhub-watchtower-v3-6:test";failure_domain="single-tencent-vps";custody_threshold=2;confirmers_file="/opt/xhub-v3.6/config/confirmers.json";custody_attesters_file="/opt/xhub-v3.6/config/attesters.json";instances=$instances;failure_domain_enforced=$false;test_only=$true;production_ready=$false;production_broadcast=$false};Attesters=$attesters}
}
function Invoke-Validation($fixtures,[switch]$Generate){
    $base=Join-Path $env:TEMP ("xhub-single-vps-docker-"+[guid]::NewGuid());New-Item -ItemType Directory -Path $base|Out-Null
    try{$profilePath=Join-Path $base "profile.json";$attestersPath=Join-Path $base "attesters.json";$fixtures.Profile|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $profilePath -Encoding utf8;$fixtures.Attesters|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $attestersPath -Encoding utf8
        $previous=$ErrorActionPreference;$ErrorActionPreference="Continue";$output=& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ProfilePath $profilePath -CustodyAttestersPath $attestersPath 2>&1;$exitCode=$LASTEXITCODE;$ErrorActionPreference=$previous
        if($Generate-and$exitCode-eq0){$out=Join-Path $base "generated";& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generator -ProfilePath $profilePath -CustodyAttestersPath $attestersPath -OutputDirectory $out *> $null;if($LASTEXITCODE-ne0){throw "Compose generation failed"};$manifest=Get-Content -LiteralPath (Join-Path $out "compose-manifest.json") -Raw|ConvertFrom-Json;$compose=Get-Content -LiteralPath (Join-Path $out "compose.yaml") -Raw;if($manifest.status-ne"SINGLE_VPS_DOCKER_COMPOSE_GENERATED_NOT_STARTED"-or$manifest.production_ready-ne$false-or$manifest.failure_domain_count-ne1){throw "Compose manifest invalid"};foreach($required in @("network_mode: host","XHUB_WATCHTOWER_LISTEN: 127.0.0.1:","read_only: true","no-new-privileges:true","cap_drop:","pids_limit: 128",'-H \"Authorization: Bearer $$TOKEN\"')){if(-not$compose.Contains($required)){throw "Compose hardening missing: $required"}};foreach($forbidden in @("0.0.0.0","ports:","privileged: true","production_ready: true","push_tx","-H 'Authorization: Bearer `$`$TOKEN'")){if($compose.Contains($forbidden)){throw "Compose contains prohibited setting: $forbidden"}}}
        [pscustomobject]@{ExitCode=$exitCode;Output=$output}
    }finally{Remove-Item -LiteralPath $base -Recurse -Force -ErrorAction SilentlyContinue}
}
$valid=Invoke-Validation (New-Fixtures) -Generate;if($valid.ExitCode-ne0-or($valid.Output|ConvertFrom-Json).status-ne"SINGLE_VPS_DOCKER_PLAN_VALIDATED_TEST_ONLY"){throw "Valid Docker test plan rejected: $($valid.Output)"}
$mutations=@({param($f)$f.Profile.failure_domain_enforced=$true},{param($f)$f.Profile.production_ready=$true},{param($f)$f.Profile.instances[1].listen_port=$f.Profile.instances[0].listen_port},{param($f)$f.Attesters[1].failure_domain="other-vps"},{param($f)$f.Attesters[1].signer_public_key=$f.Attesters[0].signer_public_key},{param($f)$f.Profile.image="xhub/watchtower:latest"})
foreach($mutation in $mutations){$fixture=New-Fixtures;&$mutation $fixture;if((Invoke-Validation $fixture).ExitCode-eq0){throw "Invalid Docker test plan accepted"}}
$dockerSource=Get-Content -LiteralPath $dockerfile -Raw
foreach($required in @("cargo build --locked --release","watchtower-monitor-aggregate-v3-6","USER 65532:65532","ENTRYPOINT")){if(-not$dockerSource.Contains($required)){throw "Dockerfile missing: $required"}}
foreach($forbidden in @("COPY . .","EXPOSE 8738","BEGIN PRIVATE KEY","push_tx")){if($dockerSource.Contains($forbidden)){throw "Dockerfile contains prohibited material: $forbidden"}}
$overlaySource=Get-Content -LiteralPath $readonlyOverlay -Raw
foreach($required in @("monitor-aggregate:","127.0.0.1:18744","watchtower-monitor-aggregate-v3-6","condition: service_healthy",'xhub.broadcast-enabled: "false"',"/var/lib/xhub-monitor-aggregate/alerts.sqlite3","/run/secrets/monitor-aggregate-api-token.txt:ro","/opt/xhub-v3.6-test/data/monitor-aggregate:/var/lib/xhub-monitor-aggregate")){if(-not$overlaySource.Contains($required)){throw "Read-only monitor overlay missing: $required"}}
foreach($forbidden in @("0.0.0.0:18744","push_tx",'production-ready: "true"')){if($overlaySource.Contains($forbidden)){throw "Read-only monitor overlay contains prohibited setting: $forbidden"}}
$installerSource=Get-Content -LiteralPath $readonlyInstaller -Raw
foreach($required in @("monitor_aggregate.rs","watchtower.Cargo.toml","xhub-v36-monitor-aggregate",".State.Health.Status","compose.readonly-monitors.yaml",'test -s "$ack_token"','install -d -o 65532 -g 65532 -m 0750 "$alert_data"','chmod 0400 "$ack_token"')){if(-not$installerSource.Contains($required)){throw "Read-only monitor installer missing: $required"}}
$failoverSource=Get-Content -LiteralPath $failoverVerifier -Raw
foreach($required in @("docker pause",'trap ''docker unpause','"status":"DEGRADED"','"agreeing_count":3',"status=AGGREGATOR_RECOVERED")){if(-not$failoverSource.Contains($required)){throw "Aggregator failover verifier missing: $required"}}
$alertVerifierSource=Get-Content -LiteralPath $alertVerifier -Raw
foreach($required in @("trap cleanup EXIT","wait_for_occurrence_increase",'Authorization: Bearer $token',"missing_token_status","wrong_token_status","wrong_protocol_status","--force-recreate --no-deps monitor-aggregate","token_present_in_logs:false","chain_broadcast:false")){if(-not$alertVerifierSource.Contains($required)){throw "Alert lifecycle verifier missing: $required"}}
foreach($forbidden in @("push_tx","production_ready:true","chain_broadcast:true")){if($alertVerifierSource.Contains($forbidden)){throw "Alert lifecycle verifier contains prohibited setting: $forbidden"}}
$tokenRotatorSource=Get-Content -LiteralPath $tokenRotator -Raw
foreach($required in @("trap rollback ERR INT TERM",'chmod 0400 "$temporary_token_file"','test "$old_status" = 401','test "$new_status" = 404',"token_present_in_output:false","chain_broadcast:false")){if(-not$tokenRotatorSource.Contains($required)){throw "Aggregate token rotator missing: $required"}}
$alertBackupSource=Get-Content -LiteralPath $alertBackup -Raw
foreach($required in @('python3 "$online_backup"','sqlite_online_backup:true','quick_check:"ok"','consistent_shutdown:false','contains_api_token:false','sha256sum "$destination/alerts.sqlite3"','[[ "$staging" == "$backup_root"/.alerts-*.tmp ]]',"chain_broadcast:false")){if(-not$alertBackupSource.Contains($required)){throw "Aggregate alert backup missing: $required"}}
$sqliteOnlineBackupSource=Get-Content -LiteralPath $sqliteOnlineBackup -Raw
foreach($required in @("source.backup(destination)",'PRAGMA quick_check',"SELECT COUNT(*) FROM v36_monitor_alert_events","destination already exists")){if(-not$sqliteOnlineBackupSource.Contains($required)){throw "SQLite online backup helper missing: $required"}}
$alertRestoreDrillSource=Get-Content -LiteralPath $alertRestoreDrill -Raw
foreach($required in @("127.0.0.1:18745","--read-only","--cap-drop ALL","no-new-privileges:true",'test "$restored_event_ids" = "$expected_event_ids"','[[ "$drill_directory" == "$root"/restore-drills/restore-drill-* ]]',"source_unchanged:true","drill_container_removed:true","chain_broadcast:false")){if(-not$alertRestoreDrillSource.Contains($required)){throw "Aggregate alert restore drill missing: $required"}}
foreach($source in @($tokenRotatorSource,$alertBackupSource,$alertRestoreDrillSource)){foreach($forbidden in @("push_tx","production_ready:true","chain_broadcast:true","0.0.0.0")){if($source.Contains($forbidden)){throw "Aggregate alert operations contain prohibited setting: $forbidden"}}}
if($alertBackupSource.Contains('docker stop')){throw "Aggregate alert backup must use SQLite Online Backup without stopping the monitor"}
$retentionSource=Get-Content -LiteralPath $retentionCandidates -Raw
foreach($required in @("keep_latest","minimum_age_seconds","automatic_deletion_enabled:false","manual_review_required:true","files_deleted:false")){if(-not$retentionSource.Contains($required)){throw "Aggregate alert retention candidate script missing: $required"}}
$inspectionSource=Get-Content -LiteralPath $operationsInspection -Raw
foreach($required in @("maximum_backup_age_seconds","capacity_gate_passed:true","systemctl is-enabled","candidate_count","physical_failure_domain_count:1","chain_broadcast:false")){if(-not$inspectionSource.Contains($required)){throw "Aggregate operations inspection missing: $required"}}
$operationsInstallerSource=Get-Content -LiteralPath $operationsInstaller -Raw
foreach($required in @("systemd-analyze verify","systemctl enable --now","systemctl start","rotate-monitor-aggregate-token.sh","verify-monitor-aggregate-alert-backup.sh","publish-monitor-aggregate-operations-inspection.sh","verify-monitor-aggregate-inspection-scheduler.sh","xhub-v36-monitor-operations-inspection.timer","publish-latest-monitor-aggregate-restore-drill.sh","verify-monitor-aggregate-restore-scheduler.sh","xhub-v36-monitor-alert-restore-drill.timer",'automatic_deletion_enabled=false','production_ready=false','chain_broadcast=false')){if(-not$operationsInstallerSource.Contains($required)){throw "Aggregate operations installer missing: $required"}}
$backupServiceSource=Get-Content -LiteralPath $backupService -Raw
foreach($required in @("Type=oneshot","NoNewPrivileges=true","ProtectSystem=strict","PrivateDevices=true","CAP_DAC_OVERRIDE","ReadOnlyPaths=/opt/xhub-v3.6-test/data/monitor-aggregate","ReadWritePaths=/opt/xhub-v3.6-test/backups/monitor-aggregate-alerts")){if(-not$backupServiceSource.Contains($required)){throw "Aggregate backup service missing: $required"}}
$backupTimerSource=Get-Content -LiteralPath $backupTimer -Raw
foreach($required in @("OnCalendar=*-*-* 03:15:00 UTC","Persistent=true","RandomizedDelaySec=15m","WantedBy=timers.target")){if(-not$backupTimerSource.Contains($required)){throw "Aggregate backup timer missing: $required"}}
$inspectionPublisherSource=Get-Content -LiteralPath $inspectionPublisher -Raw
foreach($required in @("latest.json","latest-failure.json","last-resolved-failure.json",'mv -f "$temporary" "$latest"',"automatic_remediation_enabled:false","external_notification_enabled:false","chain_broadcast:false")){if(-not$inspectionPublisherSource.Contains($required)){throw "Aggregate inspection publisher missing: $required"}}
$inspectionSchedulerVerifierSource=Get-Content -LiteralPath $inspectionSchedulerVerifier -Raw
foreach($required in @("maximum_report_age_seconds","report_atomic_publish:true","failure_marker_present:false","systemctl is-enabled","automatic_remediation_enabled:false","chain_broadcast:false")){if(-not$inspectionSchedulerVerifierSource.Contains($required)){throw "Aggregate inspection scheduler verifier missing: $required"}}
$inspectionServiceSource=Get-Content -LiteralPath $inspectionService -Raw
foreach($required in @("Type=oneshot","NoNewPrivileges=true","ProtectSystem=strict","CAP_DAC_OVERRIDE","ReadOnlyPaths=/opt/xhub-v3.6-test/backups/monitor-aggregate-alerts","ReadWritePaths=/opt/xhub-v3.6-test/operations-reports/monitor-aggregate")){if(-not$inspectionServiceSource.Contains($required)){throw "Aggregate inspection service missing: $required"}}
$inspectionTimerSource=Get-Content -LiteralPath $inspectionTimer -Raw
foreach($required in @("OnCalendar=*:0/15","Persistent=true","RandomizedDelaySec=2m","WantedBy=timers.target")){if(-not$inspectionTimerSource.Contains($required)){throw "Aggregate inspection timer missing: $required"}}
foreach($source in @($retentionSource,$inspectionSource,$operationsInstallerSource,$backupServiceSource,$backupTimerSource)){foreach($forbidden in @("push_tx","production_ready:true","chain_broadcast:true","automatic_deletion_enabled=true","0.0.0.0")){if($source.Contains($forbidden)){throw "Aggregate scheduled operations contain prohibited setting: $forbidden"}}}
foreach($source in @($inspectionPublisherSource,$inspectionSchedulerVerifierSource,$inspectionServiceSource,$inspectionTimerSource)){foreach($forbidden in @("push_tx","production_ready:true","chain_broadcast:true","automatic_remediation_enabled:true","external_notification_enabled:true","0.0.0.0")){if($source.Contains($forbidden)){throw "Aggregate inspection scheduler contains prohibited setting: $forbidden"}}}
$restorePublisherSource=Get-Content -LiteralPath $restorePublisher -Raw
foreach($required in @("latest-failure.json","last-resolved-failure.json",'mv -f "$temporary" "$latest"',"all_expected_event_ids_present","automatic_remediation_enabled:false","external_notification_enabled:false","chain_broadcast:false")){if(-not$restorePublisherSource.Contains($required)){throw "Aggregate restore publisher missing: $required"}}
$restoreSchedulerVerifierSource=Get-Content -LiteralPath $restoreSchedulerVerifier -Raw
foreach($required in @("maximum_report_age_seconds","report_atomic_publish:true","failure_marker_present:false","systemctl is-enabled","source_unchanged","chain_broadcast:false")){if(-not$restoreSchedulerVerifierSource.Contains($required)){throw "Aggregate restore scheduler verifier missing: $required"}}
$restoreServiceSource=Get-Content -LiteralPath $restoreService -Raw
foreach($required in @("Type=oneshot","NoNewPrivileges=true","ProtectSystem=strict","CAP_DAC_OVERRIDE","ReadOnlyPaths=/opt/xhub-v3.6-test/backups/monitor-aggregate-alerts","ReadWritePaths=/opt/xhub-v3.6-test/restore-drills","ReadWritePaths=/opt/xhub-v3.6-test/operations-reports/monitor-aggregate-restore")){if(-not$restoreServiceSource.Contains($required)){throw "Aggregate restore service missing: $required"}}
$restoreTimerSource=Get-Content -LiteralPath $restoreTimer -Raw
foreach($required in @("OnCalendar=Sun *-*-* 04:00:00 UTC","Persistent=true","RandomizedDelaySec=30m","WantedBy=timers.target")){if(-not$restoreTimerSource.Contains($required)){throw "Aggregate restore timer missing: $required"}}
foreach($source in @($restorePublisherSource,$restoreSchedulerVerifierSource,$restoreServiceSource,$restoreTimerSource)){foreach($forbidden in @("push_tx","production_ready:true","chain_broadcast:true","automatic_remediation_enabled:true","external_notification_enabled:true","0.0.0.0")){if($source.Contains($forbidden)){throw "Aggregate restore scheduler contains prohibited setting: $forbidden"}}}
$integrityVerifierSource=Get-Content -LiteralPath $integrityVerifier -Raw
foreach($required in @("operations-integrity-manifest.json","unmanaged integrity path","all_files_match:true","automatic_repair_enabled:false","chain_broadcast:false")){if(-not$integrityVerifierSource.Contains($required)){throw "Aggregate operations integrity verifier missing: $required"}}
$integrity=Get-Content -LiteralPath $integrityManifest -Raw|ConvertFrom-Json
if($integrity.schema-ne"xhub-v3-6-monitor-aggregate-operations-integrity-1"-or@($integrity.files).Count-ne17-or$integrity.automatic_repair_enabled-ne$false-or$integrity.chain_broadcast-ne$false){throw "Aggregate operations integrity manifest invalid"}
foreach($entry in @($integrity.files)){$sourcePath=Join-Path $PSScriptRoot ([string]$entry.source_file);if(-not(Test-Path -LiteralPath $sourcePath -PathType Leaf)){throw "Integrity source missing: $($entry.source_file)"};$actual=(Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant();if($actual-ne([string]$entry.sha256).ToLowerInvariant()){throw "Integrity source hash mismatch: $($entry.source_file)"}}
$integrityFailClosedSource=Get-Content -LiteralPath $integrityFailClosed -Raw
foreach($required in @('"00" * 32',"modified_manifest_copy_rejected:true","installed_files_modified:false","installed_manifest_modified:false","automatic_repair_enabled:false","chain_broadcast:false")){if(-not$integrityFailClosedSource.Contains($required)){throw "Integrity fail-closed verifier missing: $required"}}
$ignoreSource=Get-Content -LiteralPath $dockerignore -Raw
foreach($required in @("**/target","local-secrets","**/*.local.json","deploy/mainnet/data")){if(-not$ignoreSource.Contains($required)){throw "Docker ignore missing: $required"}}
Write-Output "SINGLE_VPS_DOCKER_TESTS_OK"
