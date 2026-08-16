param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-DeploymentConfig $ConfigPath
Assert-DeploymentConfig $deployment.Config

[pscustomobject]@{
    schema = $deployment.Config.schema
    config_path = (Resolve-Path -LiteralPath $ConfigPath).Path
    wallet_listen = $deployment.Config.wallet_listen
    hub_listen = $deployment.Config.hub_listen
    watchtower_listen = $deployment.Config.watchtower_listen
    chia_rpc_url = $deployment.Config.chia_rpc_url
    expected_network_id = $deployment.Config.expected_network_id
    status = "VALID"
} | ConvertTo-Json -Depth 4
