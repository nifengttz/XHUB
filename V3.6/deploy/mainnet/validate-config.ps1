param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-MainnetConfig $ConfigPath
$config = $deployment.Config

if ([string]$config.watchtower_recipient_id -match '^REPLACE_WITH_') {
    throw "watchtower_recipient_id must be set before mainnet preflight"
}

[pscustomobject]@{
    schema = $config.schema
    config_path = (Resolve-Path -LiteralPath $ConfigPath).Path
    wallet_listen = $config.wallet_listen
    hub_listen = $config.hub_listen
    watchtower_listen = $config.watchtower_listen
    chia_rpc_url = $config.chia_rpc_url
    rpc_mode = $config.rpc_mode
    expected_network_id = $config.expected_network_id
    funding_coin_id = $config.funding_coin_id
    broadcast_enabled = $false
    broadcast_ready = $false
    chain_broadcast = $false
    status = "VALID_READ_ONLY_PRECHECK"
} | ConvertTo-Json -Depth 4
