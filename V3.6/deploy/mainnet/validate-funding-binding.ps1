param(
    [Parameter(Mandatory = $true)][string]$RpcOutputPath,
    [Parameter(Mandatory = $true)][string]$PlanPath,
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [Parameter(Mandatory = $true)][string]$ParametersPath
)

$ErrorActionPreference = "Stop"
$rpc = Get-Content -LiteralPath (Resolve-Path $RpcOutputPath) -Raw | ConvertFrom-Json
$plan = Get-Content -LiteralPath (Resolve-Path $PlanPath) -Raw | ConvertFrom-Json
$config = Get-Content -LiteralPath (Resolve-Path $ConfigPath) -Raw | ConvertFrom-Json
$parameters = Get-Content -LiteralPath (Resolve-Path $ParametersPath) -Raw | ConvertFrom-Json
if ($rpc.schema -ne "xhub-v3-6-rpc-preflight-1" -or $rpc.ready -ne $true -or $rpc.funding_coin.status -ne "CONFIRMED") {
    throw "RPC output is not a ready confirmed Funding Coin snapshot"
}
if ([string]$plan.funding_coin_id -ne [string]$config.funding_coin_id) { throw "Configured Funding Coin ID differs from plan" }
if ([string]$rpc.network_id -ne [string]$config.expected_network_id) { throw "RPC network differs from mainnet config" }
if ([string]$plan.funding_puzzle_hash -ne [string]$rpc.funding_coin.puzzle_hash) { throw "Funding Puzzle Hash differs from plan" }
if ([uint64]$rpc.funding_coin.amount -ne [uint64]$plan.max_total_mojo) { throw "Funding Coin amount differs from 1-mojo canary plan" }
if ([uint64]$rpc.funding_coin.confirmations -lt [uint64]$parameters.funding_confirmation_blocks) { throw "Funding confirmation policy is not met" }
if ($parameters.mainnet_approved -ne $false -or $plan.broadcast_enabled -ne $false) { throw "Binding inputs changed a safety flag" }
[pscustomobject]@{
    schema = "xhub-v3-6-mainnet-funding-binding-1"
    protocol_version = "0x0360"
    plan_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $PlanPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    rpc_output_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $RpcOutputPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    parameters_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $ParametersPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    config_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $ConfigPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    funding_coin_id = $plan.funding_coin_id
    funding_puzzle_hash = $plan.funding_puzzle_hash
    funding_amount_mojo = [uint64]$rpc.funding_coin.amount
    broadcast_enabled = $false
    status = "FUNDING_BINDING_VERIFIED"
} | ConvertTo-Json -Depth 5
