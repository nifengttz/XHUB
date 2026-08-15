param(
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$CustodyAttestersPath
)

$ErrorActionPreference = "Stop"

function Assert-ExactFields($Object, [string[]]$Expected, [string]$Context) {
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "|") -ne ($expectedSorted -join "|")) {
        throw "$Context fields do not match the frozen schema"
    }
}

function Assert-Name([string]$Value, [string]$Field) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value.Length -gt 128 -or
        $Value -match '^REPLACE_WITH_' -or $Value -match '[\x00-\x1f\x7f]') {
        throw "$Field must be an explicit 1..128 character value"
    }
}

function Assert-Unique($Values, [string]$Field) {
    $items = @($Values)
    if (($items | Select-Object -Unique).Count -ne $items.Count) {
        throw "$Field values must be distinct across all Watchtowers"
    }
}

$manifest = Get-Content -LiteralPath (Resolve-Path -LiteralPath $IdentityManifestPath) -Raw | ConvertFrom-Json
$attesterConfig = @(Get-Content -LiteralPath (Resolve-Path -LiteralPath $CustodyAttestersPath) -Raw |
    ConvertFrom-Json | ForEach-Object { $_ })

Assert-ExactFields $manifest @(
    "schema", "protocol_version", "network", "merchant_receipt_public_key",
    "custody_attestation_threshold", "custody_attestation_participants",
    "attesters", "production_approved"
) "identity manifest"
if ($manifest.schema -ne "xhub-v3-6-watchtower-identities-1" -or
    $manifest.protocol_version -ne "0x0360" -or $manifest.network -ne "mainnet") {
    throw "Unsupported Watchtower identity manifest"
}
if ($manifest.production_approved -ne $false) {
    throw "Identity validation cannot grant production approval"
}
if ([int64]$manifest.custody_attestation_threshold -ne 2 -or
    [int64]$manifest.custody_attestation_participants -ne 3) {
    throw "Mainnet custody identity policy must be 2-of-3"
}
if ([string]$manifest.merchant_receipt_public_key -notmatch '^[0-9a-fA-F]{96}$') {
    throw "merchant_receipt_public_key must be 48-byte hexadecimal BLS public key"
}

$attesters = @($manifest.attesters)
if ($attesters.Count -ne 3 -or $attesterConfig.Count -ne 3) {
    throw "Identity manifest and custody attester config must each contain exactly three entries"
}
$expectedFields = @(
    "attester_id", "failure_domain", "operator_id", "infrastructure_provider", "region",
    "attester_public_key", "api_base_url", "tls_certificate_sha256"
)
foreach ($attester in $attesters) {
    Assert-ExactFields $attester $expectedFields "Watchtower identity"
    foreach ($field in @("attester_id", "failure_domain", "operator_id", "infrastructure_provider", "region")) {
        Assert-Name ([string]$attester.$field) $field
    }
    if ([string]$attester.attester_public_key -notmatch '^[0-9a-fA-F]{96}$') {
        throw "attester_public_key must be 48-byte hexadecimal BLS public key"
    }
    if ([string]$attester.attester_public_key -ieq [string]$manifest.merchant_receipt_public_key) {
        throw "Custody attester key must be distinct from the merchant receipt key"
    }
    if ([string]$attester.tls_certificate_sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "tls_certificate_sha256 must be exactly 32-byte hexadecimal"
    }
    $uri = $null
    if (-not [Uri]::TryCreate([string]$attester.api_base_url, [UriKind]::Absolute, [ref]$uri) -or
        $uri.Scheme -ne "https" -or -not [string]::IsNullOrEmpty($uri.UserInfo) -or
        -not [string]::IsNullOrEmpty($uri.Query) -or -not [string]::IsNullOrEmpty($uri.Fragment) -or
        $uri.AbsolutePath -ne "/" -or $uri.IsLoopback -or $uri.HostNameType -ne [UriHostNameType]::Dns -or
        $uri.Host -match '(?i)(^|\.)localhost$|^REPLACE_WITH_') {
        throw "api_base_url must be a public HTTPS DNS origin without path, query, or credentials"
    }
}

Assert-Unique ($attesters | ForEach-Object { ([string]$_.attester_id).ToLowerInvariant() }) "attester_id"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.failure_domain).ToLowerInvariant() }) "failure_domain"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.operator_id).ToLowerInvariant() }) "operator_id"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.infrastructure_provider).ToLowerInvariant() }) "infrastructure_provider"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.region).ToLowerInvariant() }) "region"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.attester_public_key).ToLowerInvariant() }) "attester_public_key"
Assert-Unique ($attesters | ForEach-Object { ([Uri]$_.api_base_url).DnsSafeHost.ToLowerInvariant() }) "api host"
Assert-Unique ($attesters | ForEach-Object { ([string]$_.tls_certificate_sha256).ToLowerInvariant() }) "TLS certificate fingerprint"

$configById = @{}
foreach ($entry in $attesterConfig) {
    Assert-ExactFields $entry @("signer_id", "failure_domain", "signer_public_key") "custody attester config"
    Assert-Name ([string]$entry.signer_id) "signer_id"
    if ($configById.ContainsKey(([string]$entry.signer_id).ToLowerInvariant())) {
        throw "Duplicate signer_id in custody attester config"
    }
    $configById[([string]$entry.signer_id).ToLowerInvariant()] = $entry
}
foreach ($attester in $attesters) {
    $key = ([string]$attester.attester_id).ToLowerInvariant()
    if (-not $configById.ContainsKey($key)) { throw "Identity is missing from custody attester config: $key" }
    $entry = $configById[$key]
    if ([string]$entry.failure_domain -cne [string]$attester.failure_domain -or
        [string]$entry.signer_public_key -ine [string]$attester.attester_public_key) {
        throw "Custody attester config differs from identity manifest: $key"
    }
}

[pscustomobject]@{
    schema = "xhub-v3-6-watchtower-identity-validation-1"
    protocol_version = "0x0360"
    attester_count = 3
    failure_domain_count = 3
    operator_count = 3
    provider_count = 3
    region_count = 3
    custody_attestation_policy = "2-of-3"
    app_config_matches = $true
    bls_curve_validation_deferred_to_watchtower_startup = $true
    production_approved = $false
    production_broadcast = $false
    status = "IDENTITIES_VERIFIED_CANDIDATE_ONLY"
} | ConvertTo-Json -Depth 5
