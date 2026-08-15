param(
    [Parameter(Mandatory = $true)][string]$PlanPath
)

$ErrorActionPreference = "Stop"
$plan = Get-Content -LiteralPath (Resolve-Path -LiteralPath $PlanPath) -Raw | ConvertFrom-Json

if ($plan.schema -ne "xhub-v3-6-mainnet-canary-plan-1") { throw "Unsupported canary plan schema" }
if ($plan.protocol_version -ne "0x0360") { throw "Canary plan protocol version must be 0x0360" }
if ($plan.environment -ne "mainnet-canary") { throw "Canary plan environment must be mainnet-canary" }
if ([string]$plan.funding_coin_id -notmatch '^[0-9a-fA-F]{64}$') { throw "funding_coin_id must be a real 64-hex Coin ID" }
if ([string]$plan.funding_puzzle_hash -notmatch '^[0-9a-fA-F]{64}$') { throw "funding_puzzle_hash must be a real 64-hex hash" }
if ([int64]$plan.max_total_mojo -lt 1 -or [int64]$plan.max_total_mojo -gt 1) { throw "canary max_total_mojo must be exactly 1" }
if ($plan.broadcast_enabled -ne $false) { throw "canary plan must keep broadcast_enabled=false" }
if ($plan.manual_approval_required -ne $true) { throw "canary plan requires manual approval" }

$required = @(
    "mainnet_rpc_preflight_verified",
    "funding_coin_record_verified",
    "puzzle_hash_and_module_hashes_verified",
    "recovery_package_delivery_simulated",
    "two_person_review_recorded"
)
$checks = @($plan.required_checks)
foreach ($name in $required) {
    if ($checks -notcontains $name) { throw "Missing required canary check: $name" }
}

$prohibited = @($plan.prohibited_materials)
foreach ($name in @("private_key", "mnemonic", "spend_bundle_canonical_hex", "push_tx")) {
    if ($prohibited -notcontains $name) { throw "Canary plan must prohibit material: $name" }
}

$json = $plan | ConvertTo-Json -Depth 8
if ($json -match '(?i)private.?key|mnemonic|spend.?bundle.*hex|push_tx') {
    $allowed = @('private_key', 'mnemonic', 'spend_bundle_canonical_hex', 'push_tx')
    $unexpected = $json -split "`n" | Where-Object {
        $_ -match '(?i)private.?key|mnemonic|spend.?bundle.*hex|push_tx' -and
        $_ -notmatch '"(private_key|mnemonic|spend_bundle_canonical_hex|push_tx)"'
    }
    if ($unexpected) { throw "Canary plan contains prohibited material or an unexpected sensitive field" }
}

[pscustomobject]@{
    schema = $plan.schema
    protocol_version = $plan.protocol_version
    funding_coin_id = $plan.funding_coin_id
    funding_puzzle_hash = $plan.funding_puzzle_hash
    max_total_mojo = [int64]$plan.max_total_mojo
    broadcast_enabled = $false
    manual_approval_required = $true
    status = "VALID_APPROVAL_PLAN_ONLY"
} | ConvertTo-Json -Depth 4
