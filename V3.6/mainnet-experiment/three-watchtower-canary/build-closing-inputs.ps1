param(
    [Parameter(Mandatory = $true)][string]$CandidatePath,
    [Parameter(Mandatory = $true)][string]$HubPackagePath,
    [Parameter(Mandatory = $true)][string]$ReservationPath,
    [Parameter(Mandatory = $true)][string]$RpcOutputPath,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = "Stop"

function Read-Json([string]$Path) {
    Get-Content -LiteralPath (Resolve-Path -LiteralPath $Path) -Raw | ConvertFrom-Json
}

function Write-Json([string]$Path, $Value) {
    $directory = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $json = $Value | ConvertTo-Json -Depth 20
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $json + [Environment]::NewLine, $utf8)
}

$candidate = Read-Json $CandidatePath
$package = Read-Json $HubPackagePath
$reservation = Read-Json $ReservationPath
$rpc = Read-Json $RpcOutputPath

if ($candidate.protocol_version -ne "0x0360" -or $candidate.network -ne "mainnet" -or
    [uint64]$candidate.funding_amount_mojo -ne 5 -or $candidate.mainnet_approved -ne $false -or
    $candidate.broadcast_enabled -ne $false) {
    throw "Funding candidate is not the non-broadcast 5-mojo V3.6 mainnet canary"
}
if ($package.protocol_version -ne "0x0360") {
    throw "HUB RecoveryPackage protocol version is invalid"
}
if ($package.funding_coin_id -ne $reservation.funding_coin_id -or
    $package.recovery_package_content_hash -ne $reservation.recovery_package_content_hash -or
    [uint64]$package.state_sequence -ne [uint64]$reservation.state_sequence -or
    $reservation.status -ne "SIGNED" -or $reservation.ledger_written -ne $true) {
    throw "HUB Reservation and RecoveryPackage differ"
}
if ($rpc.schema -ne "xhub-v3-6-rpc-preflight-1" -or $rpc.ready -ne $true -or
    $rpc.network_id -ne $candidate.network_id -or
    $rpc.funding_coin.status -ne "CONFIRMED" -or
    $rpc.funding_coin.puzzle_hash -ne $candidate.funding_puzzle_hash -or
    [uint64]$rpc.funding_coin.amount -ne 5 -or
    [uint64]$rpc.funding_coin.birth_height -ne 9146971) {
    throw "Coinset preflight does not match the current 5-mojo Funding Coin"
}

$plan = [ordered]@{
    schema = "xhub-v3-6-mainnet-recovery-canary-plan-1"
    protocol_version = "0x0360"
    environment = "mainnet-recovery-canary"
    funding_coin_id = $package.funding_coin_id
    funding_puzzle_hash = $candidate.funding_puzzle_hash
    max_total_mojo = 5
    initial_payment_mojo = 1
    minimum_user_remainder_mojo = 1
    broadcast_enabled = $false
    manual_approval_required = $true
    required_checks = @(
        "mainnet_rpc_preflight_verified",
        "funding_coin_record_verified",
        "puzzle_hash_and_module_hashes_verified",
        "recovery_package_generated_and_verified",
        "recovery_package_delivery_simulated",
        "two_person_review_recorded"
    )
    prohibited_materials = @("private_key", "mnemonic", "spend_bundle_canonical_hex", "push_tx")
}
$config = [ordered]@{
    schema = "xhub-v3-6-three-watchtower-closing-binding-1"
    protocol_version = "0x0360"
    rpc_mode = "trusted_public_https"
    chia_rpc_url = "https://api.coinset.org"
    expected_network_id = $candidate.network_id
    funding_coin_id = $package.funding_coin_id
    test_only = $true
    production_ready = $false
    production_broadcast = $false
}
$report = [ordered]@{
    schema = "xhub-v3-6-mainnet-recovery-package-1"
    protocol_version = "0x0360"
    network = "mainnet"
    funding_coin_id = $package.funding_coin_id
    funding_puzzle_hash = $candidate.funding_puzzle_hash
    funding_amount_mojo = 5
    state_sequence = [uint64]$package.state_sequence
    checkpoint_hash = $package.checkpoint_hash
    recovery_package_content_hash = $package.recovery_package_content_hash
    recovery_package_canonical_hex = $package.recovery_package_canonical_hex
    hub_status = $reservation.status
    ledger_written = $true
    spend_bundle_created = $false
    broadcast_enabled = $false
    chain_broadcast = $false
}

$output = [System.IO.Path]::GetFullPath($OutputDirectory)
Write-Json (Join-Path $output "recovery-canary-plan.json") $plan
Write-Json (Join-Path $output "closing-binding-config.json") $config
Write-Json (Join-Path $output "recovery-package-state-1.json") $report

[pscustomobject]@{
    funding_coin_id = $package.funding_coin_id
    state_sequence = [uint64]$package.state_sequence
    recovery_package_content_hash = $package.recovery_package_content_hash
    hypothetical_start_close_height = 9159459
    spend_bundle_created = $false
    chain_broadcast = $false
} | ConvertTo-Json -Compress
