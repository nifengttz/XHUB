param(
    [Parameter(Mandatory = $true)][string]$TlsProfilePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath
)

$ErrorActionPreference = "Stop"

function Assert-ExactFields($Object, [string[]]$Expected, [string]$Context) {
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "|") -ne ($expectedSorted -join "|")) {
        throw "$Context fields do not match the frozen schema"
    }
}

function Assert-Explicit([string]$Value, [string]$Field) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value -match '^REPLACE_WITH_' -or
        $Value -match '[\r\n]' -or $Value -match '(?i)-----BEGIN') {
        throw "$Field must be an explicit file path or identifier, never inline key material"
    }
}

$profile = Get-Content -LiteralPath (Resolve-Path -LiteralPath $TlsProfilePath) -Raw | ConvertFrom-Json
$identities = Get-Content -LiteralPath (Resolve-Path -LiteralPath $IdentityManifestPath) -Raw | ConvertFrom-Json
Assert-ExactFields $profile @("schema", "protocol_version", "nodes", "production_approved") "TLS profile"
if ($profile.schema -ne "xhub-v3-6-watchtower-tls-profile-1" -or $profile.protocol_version -ne "0x0360") {
    throw "Unsupported Watchtower TLS profile"
}
if ($profile.production_approved -ne $false) { throw "TLS profile validation cannot grant production approval" }
if ($identities.schema -ne "xhub-v3-6-watchtower-identities-1" -or $identities.protocol_version -ne "0x0360") {
    throw "Unsupported Watchtower identity manifest"
}
if ($identities.production_approved -ne $false -or $identities.network -ne "mainnet" -or
    [int64]$identities.custody_attestation_threshold -ne 2 -or
    [int64]$identities.custody_attestation_participants -ne 3) {
    throw "Watchtower identity manifest is not a fail-closed mainnet 2-of-3 candidate"
}

$nodes = @($profile.nodes)
$attesters = @($identities.attesters)
if ($nodes.Count -ne 3 -or $attesters.Count -ne 3) { throw "TLS profile and identity manifest must each contain three nodes" }
$identitiesById = @{}
foreach ($attester in $attesters) {
    $identitiesById[([string]$attester.attester_id).ToLowerInvariant()] = $attester
}

$nodeFields = @(
    "attester_id", "public_base_url", "upstream_url", "server_certificate_file",
    "server_private_key_file", "trusted_client_ca_file", "server_certificate_sha256",
    "minimum_tls_version", "client_certificate_mode", "hsts_max_age_seconds",
    "rate_limit_requests_per_minute", "max_request_body_bytes"
)
$ids = @()
foreach ($node in $nodes) {
    Assert-ExactFields $node $nodeFields "TLS node"
    Assert-Explicit ([string]$node.attester_id) "attester_id"
    $id = ([string]$node.attester_id).ToLowerInvariant()
    if (-not $identitiesById.ContainsKey($id)) { throw "TLS node has no matching Watchtower identity: $id" }
    $identity = $identitiesById[$id]
    if ([string]$node.public_base_url -cne [string]$identity.api_base_url -or
        [string]$node.server_certificate_sha256 -ine [string]$identity.tls_certificate_sha256) {
        throw "TLS endpoint or certificate fingerprint differs from identity manifest: $id"
    }
    $publicUri = $null
    if (-not [Uri]::TryCreate([string]$node.public_base_url, [UriKind]::Absolute, [ref]$publicUri) -or
        $publicUri.Scheme -ne "https" -or $publicUri.IsLoopback -or
        $publicUri.HostNameType -ne [UriHostNameType]::Dns) {
        throw "public_base_url must be a public HTTPS DNS origin"
    }
    $upstream = $null
    if (-not [Uri]::TryCreate([string]$node.upstream_url, [UriKind]::Absolute, [ref]$upstream) -or
        $upstream.Scheme -ne "http" -or -not $upstream.IsLoopback -or
        $upstream.AbsolutePath -ne "/" -or -not [string]::IsNullOrEmpty($upstream.Query) -or
        -not [string]::IsNullOrEmpty($upstream.Fragment)) {
        throw "upstream_url must be a loopback-only HTTP origin"
    }
    foreach ($field in @("server_certificate_file", "server_private_key_file", "trusted_client_ca_file")) {
        Assert-Explicit ([string]$node.$field) $field
    }
    if ([string]$node.server_certificate_file -ieq [string]$node.server_private_key_file -or
        [string]$node.server_certificate_file -ieq [string]$node.trusted_client_ca_file -or
        [string]$node.server_private_key_file -ieq [string]$node.trusted_client_ca_file) {
        throw "TLS certificate, private key, and client CA paths must be distinct"
    }
    if ([string]$node.server_certificate_sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "server_certificate_sha256 must be exactly 32-byte hexadecimal"
    }
    if ($node.minimum_tls_version -ne "TLS1.3" -or $node.client_certificate_mode -ne "require_and_verify") {
        throw "Watchtower edge must require TLS1.3 and verified client certificates"
    }
    if ([int64]$node.hsts_max_age_seconds -lt 31536000) { throw "HSTS max age must be at least one year" }
    if ([int64]$node.rate_limit_requests_per_minute -lt 1 -or
        [int64]$node.rate_limit_requests_per_minute -gt 600) {
        throw "rate_limit_requests_per_minute must be in 1..600"
    }
    if ([int64]$node.max_request_body_bytes -lt 1024 -or
        [int64]$node.max_request_body_bytes -gt 1048576) {
        throw "max_request_body_bytes must be in 1024..1048576"
    }
    $ids += $id
}
if (($ids | Select-Object -Unique).Count -ne 3) {
    throw "Each Watchtower must use a distinct identity"
}

[pscustomobject]@{
    schema = "xhub-v3-6-watchtower-tls-profile-validation-1"
    protocol_version = "0x0360"
    node_count = 3
    tls_minimum = "TLS1.3"
    mutual_tls_required = $true
    loopback_upstreams_only = $true
    certificate_pins_match_identity_manifest = $true
    live_endpoint_check_performed = $false
    private_key_content_read = $false
    production_approved = $false
    production_broadcast = $false
    status = "TLS_PROFILE_VALIDATED_CONFIG_ONLY"
} | ConvertTo-Json -Depth 5
