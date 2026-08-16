Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "..\testnet\common.ps1")

$MAINNET_SCHEMA = "xhub-v3-6-mainnet-canary-deployment-1"

function Read-MainnetConfig([string]$ConfigPath) {
    $deployment = Read-DeploymentConfig $ConfigPath $MAINNET_SCHEMA
    Assert-DeploymentConfig $deployment.Config
    $config = $deployment.Config
    $rpcUri = [Uri]$config.chia_rpc_url
    if ($rpcUri.Scheme -ne "https") {
        throw "mainnet chia_rpc_url must use https"
    }
    $rpcMode = [string]$config.rpc_mode
    if ($rpcMode -eq "trusted_public_https") {
        if ($rpcUri.AbsoluteUri.TrimEnd('/') -ne "https://api.coinset.org") {
            throw "trusted_public_https currently permits only https://api.coinset.org"
        }
        if ((Get-OptionalConfigValue $config "chia_rpc_cert_file") -or
            (Get-OptionalConfigValue $config "chia_rpc_key_file")) {
            throw "trusted public RPC must not configure a client certificate or key"
        }
    } elseif ($rpcMode -eq "self_hosted_mtls") {
        if (-not (Get-OptionalConfigValue $config "chia_rpc_cert_file") -or
            -not (Get-OptionalConfigValue $config "chia_rpc_key_file")) {
            throw "self_hosted_mtls requires both RPC certificate and key files"
        }
    } else {
        throw "rpc_mode must be trusted_public_https or self_hosted_mtls"
    }
    if ([string]$config.expected_network_id -eq ("a" * 64)) {
        throw "mainnet expected_network_id must not use the smoke-test value"
    }
    if ([string]$config.funding_coin_id -notmatch '^[0-9a-fA-F]{64}$') {
        throw "mainnet funding_coin_id must be a real 64-hex Coin ID"
    }
    [pscustomobject]@{ Config = $config; Directory = $deployment.Directory }
}
