$ErrorActionPreference = "Stop"
$auditScript = Join-Path $PSScriptRoot "audit-source-control-readiness.ps1"
$verifyScript = Join-Path $PSScriptRoot "verify-source-control-candidate-manifest.ps1"
$audit = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $auditScript | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Source-control readiness audit failed" }
$verified = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $verifyScript | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Candidate manifest verification failed" }
if ($verified.status -ne "VERIFIED" -or $verified.all_entries_match -ne $true) {
    throw "Candidate manifest is not verified"
}
if ($verified.candidate_file_count -ne $audit.candidate_file_count) {
    throw "Candidate manifest and readiness counts differ"
}
if ($verified.candidate_total_bytes -ne $audit.candidate_total_bytes) {
    throw "Candidate manifest and readiness byte counts differ"
}
if ([string]$verified.tree_sha256 -notmatch '^[0-9a-f]{64}$') {
    throw "Candidate tree hash is not canonical SHA-256"
}
if ($verified.commit_created -ne $false -or $verified.automatic_staging_enabled -ne $false) {
    throw "Candidate verification must not mutate Git"
}
if ($verified.production_ready -ne $false -or $verified.chain_broadcast -ne $false) {
    throw "Candidate verification weakened safety boundaries"
}
Write-Output "SOURCE_CONTROL_CANDIDATE_MANIFEST_TESTS_OK"
