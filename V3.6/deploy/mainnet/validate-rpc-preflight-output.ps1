param(
    [Parameter(Mandatory = $true)][string]$RpcOutputPath,
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

$ErrorActionPreference = "Stop"
$rpcText = Get-Content -LiteralPath (Resolve-Path -LiteralPath $RpcOutputPath) -Raw
if ($rpcText -match '(?i)private.?key|mnemonic|spend.?bundle|push_tx') { throw "RPC evidence contains prohibited material" }
$rpc = $rpcText | ConvertFrom-Json
$config = Get-Content -LiteralPath (Resolve-Path -LiteralPath $ConfigPath) -Raw | ConvertFrom-Json
if ($rpc.schema -ne "xhub-v3-6-rpc-preflight-1" -or $rpc.protocol_version -ne "0x0360") { throw "Unsupported RPC preflight output" }
if ([string]$config.expected_network_id -ne [string]$rpc.network_id) { throw "RPC network_id differs from mainnet config" }
if ($rpc.synced -ne $true -or $null -eq $rpc.peak_height) { throw "Full Node is not synced with a peak" }
if ($rpc.funding_coin.status -ne "CONFIRMED") { throw "Funding Coin is not confirmed and unspent" }
if ([string]$rpc.funding_coin.puzzle_hash -notmatch '^[0-9a-fA-F]{64}$') { throw "Funding Coin puzzle hash is invalid" }
if ([uint64]$rpc.funding_coin.amount -lt 1) { throw "Funding Coin amount must be positive" }
if ([uint64]$rpc.funding_coin.confirmations -lt [uint64]$rpc.required_funding_confirmations -or $rpc.ready -ne $true) {
    throw "Funding Coin has not reached the required confirmation depth"
}
[pscustomobject]@{
    schema = "xhub-v3-6-mainnet-rpc-evidence-1"
    protocol_version = "0x0360"
    rpc_output_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $RpcOutputPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    network_id = $rpc.network_id
    funding_coin_id = $config.funding_coin_id
    funding_puzzle_hash = $rpc.funding_coin.puzzle_hash
    funding_amount_mojo = [uint64]$rpc.funding_coin.amount
    confirmations = [uint64]$rpc.funding_coin.confirmations
    broadcast_enabled = $false
    status = "RPC_PREFLIGHT_EVIDENCE_VERIFIED"
} | ConvertTo-Json -Depth 5
