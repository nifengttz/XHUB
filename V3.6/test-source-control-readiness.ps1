$ErrorActionPreference = "Stop"
$audit = Join-Path $PSScriptRoot "audit-source-control-readiness.ps1"
$result = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $audit | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Source-control readiness audit failed" }
if ($result.status -ne "READY_FOR_MANUAL_GIT_REVIEW") { throw "Unexpected readiness status" }
if ($result.candidate_file_count -lt 1) { throw "Candidate set is empty" }
if ($result.candidate_count_excludes_report -ne $true) { throw "Readiness report count is not stable" }
if ($result.all_sensitive_probes_ignored -ne $true) { throw "Sensitive ignore probes failed" }
if ($result.prohibited_candidate_count -ne 0) { throw "Prohibited candidates remain" }
if ($result.private_key_marker_hit_count -ne 0) { throw "Private-key marker found" }
if ($result.private_payloads_included -ne $false -or
    $result.local_secrets_included -ne $false -or
    $result.generated_sqlite_included -ne $false) {
    throw "Sensitive or generated payload is included"
}
if ($result.commit_created -ne $false -or $result.automatic_staging_enabled -ne $false) {
    throw "Audit must not stage or commit files"
}
if ($result.production_ready -ne $false -or $result.chain_broadcast -ne $false) {
    throw "Audit weakened production or broadcast boundaries"
}
Write-Output "SOURCE_CONTROL_READINESS_TESTS_OK"
