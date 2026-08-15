param(
    [Parameter(Mandatory = $true)][string]$PlanPath
)

$ErrorActionPreference = "Stop"
$planText = Get-Content -LiteralPath (Resolve-Path -LiteralPath $PlanPath) -Raw
$unexpectedSensitiveLines = $planText -split "`n" | Where-Object {
    $_ -match '(?i)private.?key|mnemonic|spend.?bundle.*hex|push_tx' -and
    $_ -notmatch '"(private_key|mnemonic|spend_bundle_canonical_hex|push_tx)"'
}
if ($unexpectedSensitiveLines) { throw "Recovery canary plan contains prohibited material" }
$plan = $planText | ConvertFrom-Json

if ($plan.schema -ne "xhub-v3-6-mainnet-recovery-canary-plan-1") { throw "Unsupported recovery canary plan schema" }
if ($plan.protocol_version -ne "0x0360") { throw "Recovery canary plan protocol version must be 0x0360" }
if ($plan.environment -ne "mainnet-recovery-canary") { throw "Recovery canary plan environment must be mainnet-recovery-canary" }
if ([string]$plan.funding_coin_id -notmatch '^[0-9a-fA-F]{64}$') { throw "funding_coin_id must be a real 64-hex Coin ID" }
if ([string]$plan.funding_puzzle_hash -notmatch '^[0-9a-fA-F]{64}$') { throw "funding_puzzle_hash must be a real 64-hex hash" }
if ([uint64]$plan.max_total_mojo -ne 5) { throw "recovery canary funding amount must be exactly 5 mojo" }
if ([uint64]$plan.initial_payment_mojo -ne 1) { throw "recovery canary initial payment must be exactly 1 mojo" }
if ([uint64]$plan.minimum_user_remainder_mojo -lt 1) { throw "recovery canary must retain a positive user remainder" }
if ([uint64]$plan.initial_payment_mojo + [uint64]$plan.minimum_user_remainder_mojo -gt [uint64]$plan.max_total_mojo) {
    throw "recovery canary payment and minimum remainder exceed funding amount"
}
if ($plan.broadcast_enabled -ne $false) { throw "recovery canary plan must keep broadcast_enabled=false" }
if ($plan.manual_approval_required -ne $true) { throw "recovery canary plan requires manual approval" }

$required = @(
    "mainnet_rpc_preflight_verified",
    "funding_coin_record_verified",
    "puzzle_hash_and_module_hashes_verified",
    "recovery_package_generated_and_verified",
    "recovery_package_delivery_simulated",
    "two_person_review_recorded"
)
foreach ($name in $required) {
    if (@($plan.required_checks) -notcontains $name) { throw "Missing required recovery canary check: $name" }
}
foreach ($name in @("private_key", "mnemonic", "spend_bundle_canonical_hex", "push_tx")) {
    if (@($plan.prohibited_materials) -notcontains $name) { throw "Recovery canary plan must prohibit material: $name" }
}

[pscustomobject]@{
    schema = $plan.schema
    protocol_version = $plan.protocol_version
    funding_coin_id = $plan.funding_coin_id
    funding_puzzle_hash = $plan.funding_puzzle_hash
    funding_amount_mojo = [uint64]$plan.max_total_mojo
    initial_payment_mojo = [uint64]$plan.initial_payment_mojo
    minimum_user_remainder_mojo = [uint64]$plan.minimum_user_remainder_mojo
    broadcast_enabled = $false
    manual_approval_required = $true
    status = "VALID_RECOVERY_CANARY_PLAN_ONLY"
} | ConvertTo-Json -Depth 5
