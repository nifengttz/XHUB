param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-MainnetConfig $ConfigPath
$config = $deployment.Config
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent

Remove-Item Env:XHUB_CHIA_RPC_CERT_FILE,Env:XHUB_CHIA_RPC_KEY_FILE -ErrorAction SilentlyContinue
$env:XHUB_CHIA_RPC_URL = $config.chia_rpc_url
if ($config.rpc_mode -eq "self_hosted_mtls") {
    $env:XHUB_CHIA_RPC_CERT_FILE = Resolve-ConfigPath $deployment.Directory $config.chia_rpc_cert_file
    $env:XHUB_CHIA_RPC_KEY_FILE = Resolve-ConfigPath $deployment.Directory $config.chia_rpc_key_file
}
$env:XHUB_EXPECTED_NETWORK_ID = $config.expected_network_id
$env:XHUB_PREFLIGHT_FUNDING_COIN_ID = $config.funding_coin_id

try {
    cargo run --offline --manifest-path (Join-Path $root "hub-v3_6/Cargo.toml") --bin xhub-rpc-preflight
    if ($LASTEXITCODE -ne 0) { throw "Mainnet RPC preflight failed" }
} finally {
    Remove-Item Env:XHUB_CHIA_RPC_URL,Env:XHUB_CHIA_RPC_CERT_FILE,Env:XHUB_CHIA_RPC_KEY_FILE,Env:XHUB_EXPECTED_NETWORK_ID,Env:XHUB_PREFLIGHT_FUNDING_COIN_ID -ErrorAction SilentlyContinue
}
