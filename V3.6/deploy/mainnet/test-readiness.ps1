$ErrorActionPreference = "Stop"
$evaluator = Join-Path $PSScriptRoot "evaluate-readiness.ps1"
$example = Join-Path $PSScriptRoot "readiness-input.example.json"
$blocked = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $evaluator -InputPath $example | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $blocked.status -ne "BLOCKED_EXTERNAL_INPUT" -or
    $blocked.pending_count -ne 15 -or $blocked.production_broadcast -ne $false) {
    throw "Pending readiness input did not fail closed"
}
$ready = Get-Content -LiteralPath $example -Raw | ConvertFrom-Json
$ready.config_validation = "VALID_READ_ONLY_PRECHECK"
$ready.rpc_preflight = "VERIFIED"
$ready.funding_binding = "FUNDING_BINDING_VERIFIED"
$ready.recovery_simulation = "RECOVERY_SIMULATION_VERIFIED"
$ready.canary_plan_validation = "VALID_APPROVAL_PLAN_ONLY"
$ready.canary_evidence_validation = "VALID_EVIDENCE_ONLY"
$ready.two_person_review = "TWO_PERSON_REVIEW_VERIFIED"
$ready.secret_isolation = "SECRETS_ISOLATED"
$ready.identity_manifest_validation = "IDENTITIES_VERIFIED_CANDIDATE_ONLY"
$ready.tls_profile_validation = "TLS_PROFILE_VALIDATED_CONFIG_ONLY"
$ready.runtime_plan_validation = "WATCHTOWER_RUNTIME_PLAN_VALIDATED_ONLY"
$ready.deployment_config_validation = "DEPLOYMENT_CONFIGS_VERIFIED_NOT_INSTALLED"
$ready.watchtower_deployment_verification = "THREE_OPERATORS_VERIFIED"
$ready.tls_endpoint_verification = "TLS_ENDPOINTS_VERIFIED"
$ready.external_security_review = "APPROVED"
$path = Join-Path $env:TEMP ("xhub-readiness-" + [guid]::NewGuid() + ".json")
try {
    $ready | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
    $report = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $evaluator -InputPath $path | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $report.status -ne "READY_FOR_MANUAL_REVIEW" -or $report.production_broadcast -ne $false) {
        throw "Complete readiness input did not reach manual review"
    }
} finally {
    Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
}
Write-Output "READINESS_TESTS_OK"
