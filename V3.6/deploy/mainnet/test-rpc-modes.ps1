$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

function New-Config {
    [pscustomobject]@{
        schema = "xhub-v3-6-mainnet-canary-deployment-1"
        wallet_listen = "127.0.0.1:8736"
        hub_listen = "127.0.0.1:8737"
        watchtower_listen = "127.0.0.1:8738"
        hub_db = "./data/hub.sqlite3"
        watchtower_db = "./data/watchtower.sqlite3"
        hub_api_token_file = "./secrets/hub-token.txt"
        watchtower_api_token_file = "./secrets/watchtower-token.txt"
        hub_bls_secret_file = "./secrets/hub-bls.hex"
        watchtower_confirmers_file = "./confirmers.json"
        watchtower_custody_attesters_file = "./custody-attesters.json"
        watchtower_url = "http://127.0.0.1:8738"
        watchtower_recipient_id = "watchtower-mainnet-1"
        rpc_mode = "trusted_public_https"
        chia_rpc_url = "https://api.coinset.org"
        chia_rpc_cert_file = $null
        chia_rpc_key_file = $null
        expected_network_id = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb"
        funding_coin_id = "d8d089881dde12de0bdb8a078df9ab047da307d3f671b5b188b78448d570ea9d"
    }
}

function Test-Config($config) {
    $path = Join-Path $env:TEMP ("xhub-rpc-mode-" + [guid]::NewGuid() + ".json")
    try {
        $config | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try { Read-MainnetConfig $path *> $null; $accepted = $true } catch { $accepted = $false }
        $ErrorActionPreference = $previous
        $accepted
    } finally { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}

if (-not (Test-Config (New-Config))) { throw "Trusted Coinset public HTTPS config was rejected" }
$wrongHost = New-Config; $wrongHost.chia_rpc_url = "https://example.com"
if (Test-Config $wrongHost) { throw "Unlisted trusted public RPC was accepted" }
$publicWithKey = New-Config; $publicWithKey.chia_rpc_key_file = "./rpc.key"
if (Test-Config $publicWithKey) { throw "Trusted public RPC accepted a client key" }
$mtls = New-Config; $mtls.rpc_mode = "self_hosted_mtls"; $mtls.chia_rpc_url = "https://rpc.example.com"; $mtls.chia_rpc_cert_file = "./rpc.crt"; $mtls.chia_rpc_key_file = "./rpc.key"
if (-not (Test-Config $mtls)) { throw "Self-hosted mTLS config was rejected" }
$mtls.chia_rpc_key_file = $null
if (Test-Config $mtls) { throw "Incomplete mTLS config was accepted" }

Write-Output "RPC_MODE_TESTS_OK"
