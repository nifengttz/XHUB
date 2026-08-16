Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Read-DeploymentConfig([string]$ConfigPath, [string]$ExpectedSchema = "xhub-v3-6-testnet-deployment-1") {
    $resolved = (Resolve-Path -LiteralPath $ConfigPath).Path
    $config = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
    if ($config.schema -ne $ExpectedSchema) {
        throw "Unsupported deployment schema: $($config.schema)"
    }
    [pscustomobject]@{ Config = $config; Directory = Split-Path $resolved -Parent }
}

function Resolve-ConfigPath([string]$Directory, [string]$Value) {
    if ([System.IO.Path]::IsPathRooted($Value)) {
        return [System.IO.Path]::GetFullPath($Value)
    }
    [System.IO.Path]::GetFullPath((Join-Path $Directory $Value))
}

function Read-SecretFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Secret file is missing: $Path"
    }
    $secret = (Get-Content -LiteralPath $Path -Raw).Trim()
    if ($secret.Length -lt 32) {
        throw "Secret file must contain at least 32 characters: $Path"
    }
    $secret
}

function Assert-Loopback([string]$Name, [string]$Address) {
    if (-not ($Address.StartsWith("127.0.0.1:") -or $Address.StartsWith("[::1]:"))) {
        throw "$Name must use a loopback listen address"
    }
}

function Get-OptionalConfigValue($Config, [string]$Name) {
    $property = $Config.PSObject.Properties[$Name]
    if ($null -eq $property -or [string]::IsNullOrWhiteSpace([string]$property.Value)) {
        return $null
    }
    [string]$property.Value
}

function Assert-DeploymentConfig($Config) {
    $required = @(
        "schema", "wallet_listen", "hub_listen", "watchtower_listen",
        "hub_db", "watchtower_db", "hub_api_token_file",
        "watchtower_api_token_file", "hub_bls_secret_file",
        "watchtower_confirmers_file", "watchtower_url",
        "watchtower_recipient_id", "chia_rpc_url", "expected_network_id"
    )
    foreach ($name in $required) {
        $property = $Config.PSObject.Properties[$name]
        if ($null -eq $property -or [string]::IsNullOrWhiteSpace([string]$property.Value)) {
            throw "Deployment config field is required: $name"
        }
    }

    $listenNames = @("wallet_listen", "hub_listen", "watchtower_listen")
    $ports = @{}
    foreach ($name in $listenNames) {
        Assert-Loopback $name ([string]$Config.$name)
        if ([string]$Config.$name -notmatch '^(127\.0\.0\.1|\[::1\]):([1-9][0-9]{0,4})$') {
            throw "$name must use an explicit loopback IPv4/IPv6 address and port"
        }
        $port = [int]([regex]::Match([string]$Config.$name, '([0-9]+)$').Value)
        if ($port -gt 65535) { throw "$name port is out of range" }
        if ($ports.ContainsKey($port)) { throw "listen ports must be unique: $port" }
        $ports[$port] = $name
    }

    $rpcUri = $null
    if (-not [Uri]::TryCreate([string]$Config.chia_rpc_url, [UriKind]::Absolute, [ref]$rpcUri)) {
        throw "chia_rpc_url must be an absolute HTTP(S) URL"
    }
    if ($rpcUri.Scheme -notin @("http", "https")) {
        throw "chia_rpc_url must use http or https"
    }
    if ($rpcUri.Scheme -eq "http" -and -not $rpcUri.IsLoopback) {
        throw "non-loopback chia_rpc_url must use https"
    }
    $rpcCert = Get-OptionalConfigValue $Config "chia_rpc_cert_file"
    $rpcKey = Get-OptionalConfigValue $Config "chia_rpc_key_file"
    if (($null -eq $rpcCert) -ne ($null -eq $rpcKey)) {
        throw "RPC certificate and key must be configured together"
    }
    if ([string]$Config.expected_network_id -notmatch '^[0-9a-fA-F]{64}$') {
        throw "expected_network_id must be exactly 64 hexadecimal characters"
    }
    if ([string]$Config.watchtower_url -notmatch '^http://(127\.0\.0\.1|\[::1\]):[1-9][0-9]{0,4}$') {
        throw "watchtower_url must point to a loopback HTTP endpoint"
    }
    $expectedWatchtowerUrl = "http://$($Config.watchtower_listen)"
    if ([string]$Config.watchtower_url -ne $expectedWatchtowerUrl) {
        throw "watchtower_url must match watchtower_listen ($expectedWatchtowerUrl)"
    }
    if ([string]$Config.hub_db -eq [string]$Config.watchtower_db) {
        throw "hub_db and watchtower_db must be different paths"
    }
    if ([string]$Config.hub_api_token_file -eq [string]$Config.watchtower_api_token_file) {
        throw "hub and watchtower API token files must be different"
    }
    if ([string]$Config.hub_bls_secret_file -eq [string]$Config.hub_api_token_file -or
        [string]$Config.hub_bls_secret_file -eq [string]$Config.watchtower_api_token_file) {
        throw "HUB BLS secret file must be separate from API token files"
    }
}
