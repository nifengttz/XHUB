$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")

function New-TestConfig {
    [pscustomobject]@{
        schema = "xhub-v3-6-testnet-deployment-1"
        wallet_listen = "127.0.0.1:18736"
        hub_listen = "127.0.0.1:18737"
        watchtower_listen = "127.0.0.1:18738"
        hub_db = "./data/hub.sqlite3"
        watchtower_db = "./data/watchtower.sqlite3"
        hub_api_token_file = "./secrets/hub-token.txt"
        watchtower_api_token_file = "./secrets/watchtower-token.txt"
        hub_bls_secret_file = "./secrets/hub-bls.hex"
        watchtower_confirmers_file = "./confirmers.json"
        watchtower_url = "http://127.0.0.1:18738"
        watchtower_recipient_id = "watchtower-test-1"
        chia_rpc_url = "https://rpc.testnet.example"
        chia_rpc_cert_file = $null
        chia_rpc_key_file = $null
        expected_network_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
}

function Assert-Rejected([string]$Name, [scriptblock]$Mutation) {
    $config = New-TestConfig
    & $Mutation $config
    try {
        Assert-DeploymentConfig $config
    } catch {
        return
    }
    throw "Invalid deployment config was accepted: $Name"
}

Assert-DeploymentConfig (New-TestConfig)
Assert-Rejected "public listener" { param($config) $config.hub_listen = "0.0.0.0:18737" }
Assert-Rejected "duplicate port" { param($config) $config.hub_listen = $config.wallet_listen }
Assert-Rejected "placeholder network ID" { param($config) $config.expected_network_id = "REPLACE_WITH_NETWORK_ID" }
Assert-Rejected "wrong watchtower endpoint" { param($config) $config.watchtower_url = "http://127.0.0.1:9999" }
Assert-Rejected "shared database path" { param($config) $config.watchtower_db = $config.hub_db }
Assert-Rejected "shared API token" { param($config) $config.watchtower_api_token_file = $config.hub_api_token_file }
Assert-Rejected "non-HTTP RPC URL" { param($config) $config.chia_rpc_url = "ftp://rpc.testnet.example" }
Assert-Rejected "cleartext remote RPC URL" { param($config) $config.chia_rpc_url = "http://rpc.testnet.example" }
Assert-Rejected "unpaired RPC certificate" { param($config) $config.chia_rpc_cert_file = "./full-node.crt" }

Write-Output "CONFIG_VALIDATION_TESTS_OK"
