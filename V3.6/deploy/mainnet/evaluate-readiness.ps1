param(
    [Parameter(Mandatory = $true)][string]$InputPath
)

$ErrorActionPreference = "Stop"
$input = Get-Content -LiteralPath (Resolve-Path -LiteralPath $InputPath) -Raw | ConvertFrom-Json
if ($input.schema -ne "xhub-v3-6-mainnet-readiness-input-1" -or $input.protocol_version -ne "0x0360") {
    throw "Unsupported readiness input"
}
if ($input.production_broadcast -ne $false) { throw "Readiness input cannot enable production broadcast" }
$expected = [ordered]@{
    parameter_validation = "VALID_CANDIDATE_ONLY"
    artifact_validation = "ARTIFACTS_VERIFIED"
    config_validation = "VALID_READ_ONLY_PRECHECK"
    rpc_preflight = "VERIFIED"
    funding_binding = "FUNDING_BINDING_VERIFIED"
    recovery_simulation = "RECOVERY_SIMULATION_VERIFIED"
    canary_plan_validation = "VALID_APPROVAL_PLAN_ONLY"
    canary_evidence_validation = "VALID_EVIDENCE_ONLY"
    two_person_review = "TWO_PERSON_REVIEW_VERIFIED"
    secret_isolation = "SECRETS_ISOLATED"
    identity_manifest_validation = "IDENTITIES_VERIFIED_CANDIDATE_ONLY"
    tls_profile_validation = "TLS_PROFILE_VALIDATED_CONFIG_ONLY"
    runtime_plan_validation = "WATCHTOWER_RUNTIME_PLAN_VALIDATED_ONLY"
    deployment_config_validation = "DEPLOYMENT_CONFIGS_VERIFIED_NOT_INSTALLED"
    watchtower_deployment_verification = "THREE_OPERATORS_VERIFIED"
    tls_endpoint_verification = "TLS_ENDPOINTS_VERIFIED"
    external_security_review = "APPROVED"
}
$checks = @()
foreach ($entry in $expected.GetEnumerator()) {
    $actual = [string]$input.($entry.Key)
    $checks += [ordered]@{ name = $entry.Key; expected = $entry.Value; actual = $actual; passed = ($actual -eq $entry.Value) }
}
$pending = @($checks | Where-Object { -not $_.passed })
$status = if ($pending.Count -eq 0) { "READY_FOR_MANUAL_REVIEW" } else { "BLOCKED_EXTERNAL_INPUT" }
[pscustomobject]@{
    schema = "xhub-v3-6-mainnet-readiness-report-1"
    protocol_version = "0x0360"
    checks = $checks
    pending_count = $pending.Count
    production_broadcast = $false
    status = $status
} | ConvertTo-Json -Depth 8
