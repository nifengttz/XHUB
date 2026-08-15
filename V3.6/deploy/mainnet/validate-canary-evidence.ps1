param(
    [Parameter(Mandatory = $true)][string]$EvidencePath,
    [Parameter(Mandatory = $true)][string]$PlanPath,
    [string]$ModuleManifestPath = "..\..\puzzles-v3_6\module-hashes.json"
)

$ErrorActionPreference = "Stop"
$evidencePathResolved = (Resolve-Path -LiteralPath $EvidencePath).Path
$planPathResolved = (Resolve-Path -LiteralPath $PlanPath).Path
$evidenceText = Get-Content -LiteralPath $evidencePathResolved -Raw
$planText = Get-Content -LiteralPath $planPathResolved -Raw

if ($evidenceText -match '(?i)private.?key|mnemonic|spend.?bundle.*hex|push_tx') {
    throw "Evidence package contains prohibited secret or broadcast material"
}
$evidence = $evidenceText | ConvertFrom-Json
$plan = $planText | ConvertFrom-Json

if ($evidence.schema -ne "xhub-v3-6-mainnet-canary-evidence-1") { throw "Unsupported evidence schema" }
if ($evidence.protocol_version -ne "0x0360" -or $plan.protocol_version -ne "0x0360") { throw "Evidence and plan must use protocol 0x0360" }
if ($plan.schema -ne "xhub-v3-6-mainnet-canary-plan-1") { throw "Unsupported canary plan schema" }
$planHash = (Get-FileHash -LiteralPath $planPathResolved -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$evidence.plan_sha256 -ne $planHash) { throw "Evidence is not bound to the approved canary plan" }

foreach ($name in @("network_id", "funding_coin_id", "funding_puzzle_hash")) {
    if ([string]$evidence.$name -notmatch '^[0-9a-fA-F]{64}$') { throw "$name must be a real 64-hex value" }
}
if ([string]$plan.funding_coin_id -ne [string]$evidence.funding_coin_id) { throw "Funding Coin differs from approved plan" }
if ([string]$plan.funding_puzzle_hash -ne [string]$evidence.funding_puzzle_hash) { throw "Funding Puzzle Hash differs from approved plan" }
if ($evidence.preflight.status -ne "VERIFIED") { throw "Mainnet RPC preflight is not VERIFIED" }
if ([string]$evidence.preflight.snapshot_hash -notmatch '^[0-9a-fA-F]{64}$') { throw "preflight snapshot_hash must be 64 hex" }
if ([int64]$evidence.preflight.funding_coin_confirmations -lt 0) { throw "Funding confirmations cannot be negative" }
if ($evidence.recovery_simulation.status -ne "VERIFIED" -or
    [string]$evidence.recovery_simulation.package_content_hash -notmatch '^[0-9a-fA-F]{64}$') {
    throw "RecoveryPackage simulation evidence is incomplete"
}
if ($evidence.broadcast_enabled -ne $false -or $evidence.external_broadcast_authorized -ne $false) {
    throw "Evidence package cannot authorize broadcast"
}

$manifestCandidate = if ([System.IO.Path]::IsPathRooted($ModuleManifestPath)) {
    $ModuleManifestPath
} else {
    Join-Path $PSScriptRoot $ModuleManifestPath
}
$manifestPathResolved = (Resolve-Path -LiteralPath $manifestCandidate).Path
$manifest = Get-Content -LiteralPath $manifestPathResolved -Raw | ConvertFrom-Json
if ($manifest.protocol_version -ne "0x0360") { throw "Module manifest protocol version mismatch" }
foreach ($property in $manifest.modules.PSObject.Properties) {
    $actual = [string]$evidence.module_hashes.($property.Name)
    $expected = [string]$property.Value.module_hash
    if ($actual -notmatch '^[0-9a-fA-F]{64}$' -or $actual.ToLowerInvariant() -ne $expected.ToLowerInvariant()) {
        throw "CLVM module hash mismatch: $($property.Name)"
    }
}

$approvals = @($evidence.approval_records)
if ($approvals.Count -ne 2) { throw "Exactly two approval records are required" }
if (($approvals | Where-Object { $_.status -ne "APPROVED" }).Count -gt 0) { throw "All approval records must be APPROVED" }
if (($approvals | Select-Object -ExpandProperty reviewer_id -Unique).Count -ne 2) { throw "Approval reviewer IDs must be distinct" }
if (($approvals | Select-Object -ExpandProperty failure_domain -Unique).Count -ne 2) { throw "Approval failure domains must be distinct" }

[pscustomobject]@{
    schema = $evidence.schema
    protocol_version = $evidence.protocol_version
    plan_sha256 = $planHash
    funding_coin_id = $evidence.funding_coin_id
    funding_puzzle_hash = $evidence.funding_puzzle_hash
    approvals = $approvals.Count
    broadcast_enabled = $false
    external_broadcast_authorized = $false
    status = "VALID_EVIDENCE_ONLY"
} | ConvertTo-Json -Depth 4
