param(
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [Parameter(Mandatory = $true)][string]$FundingCoinId
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-DeploymentConfig $ConfigPath
$config = $deployment.Config
Assert-DeploymentConfig $config
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent

$env:XHUB_CHIA_RPC_URL = $config.chia_rpc_url
$rpcCert = Get-OptionalConfigValue $config "chia_rpc_cert_file"
$rpcKey = Get-OptionalConfigValue $config "chia_rpc_key_file"
if (($null -eq $rpcCert) -ne ($null -eq $rpcKey)) { throw "RPC certificate and key must be configured together" }
if ($rpcCert) {
    $env:XHUB_CHIA_RPC_CERT_FILE = Resolve-ConfigPath $deployment.Directory $rpcCert
    $env:XHUB_CHIA_RPC_KEY_FILE = Resolve-ConfigPath $deployment.Directory $rpcKey
}
$env:XHUB_EXPECTED_NETWORK_ID = $config.expected_network_id
$env:XHUB_PREFLIGHT_FUNDING_COIN_ID = $FundingCoinId

try {
    cargo run --offline --manifest-path (Join-Path $root "hub-v3_6/Cargo.toml") --bin xhub-rpc-preflight
    if ($LASTEXITCODE -ne 0) { throw "RPC preflight failed" }
} finally {
    Remove-Item Env:XHUB_CHIA_RPC_URL,Env:XHUB_CHIA_RPC_CERT_FILE,Env:XHUB_CHIA_RPC_KEY_FILE,Env:XHUB_EXPECTED_NETWORK_ID,Env:XHUB_PREFLIGHT_FUNDING_COIN_ID -ErrorAction SilentlyContinue
}
