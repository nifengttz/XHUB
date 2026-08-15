param(
    [Parameter(Mandatory = $true)][string]$ProbeProfilePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$TlsProfilePath,
    [switch]$PlanOnly
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http
if ($null -eq ("XhubPinnedHttpClientHandler" -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Net.Http;
using System.Net.Security;
using System.Security.Authentication;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;

public sealed class XhubPinnedHttpClientHandler : HttpClientHandler
{
    public string ObservedCertificateSha256 { get; private set; }
    public bool CertificateAccepted { get; private set; }

    public XhubPinnedHttpClientHandler(string expectedCertificateSha256)
    {
        SslProtocols = SslProtocols.Tls13;
        CheckCertificateRevocationList = true;
        ServerCertificateCustomValidationCallback = (request, certificate, chain, errors) =>
        {
            if (certificate == null) return false;
            using (SHA256 sha = SHA256.Create())
            {
                byte[] hash = sha.ComputeHash(certificate.RawData);
                ObservedCertificateSha256 = BitConverter.ToString(hash).Replace("-", "").ToLowerInvariant();
            }
            CertificateAccepted = errors == SslPolicyErrors.None &&
                String.Equals(ObservedCertificateSha256, expectedCertificateSha256, StringComparison.OrdinalIgnoreCase);
            return CertificateAccepted;
        };
    }
}
"@ -ReferencedAssemblies System.Net.Http
}

function Assert-ExactFields($Object, [string[]]$Expected, [string]$Context) {
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "|") -ne ($expectedSorted -join "|")) { throw "$Context fields do not match the frozen schema" }
}

function Assert-Hex([string]$Value, [int]$Length, [string]$Field) {
    if ($Value -notmatch "^[0-9a-fA-F]{$Length}$") { throw "$Field must be exactly $Length hexadecimal characters" }
}

function Assert-ExplicitPath([string]$Value, [string]$Field) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value -match '^REPLACE_WITH_' -or
        $Value -match '[\r\n]' -or $Value -match '(?i)-----BEGIN') {
        throw "$Field must be an explicit file path without inline secret material"
    }
}

function Resolve-ProbePath([string]$BaseDirectory, [string]$Value) {
    $candidate = if ([System.IO.Path]::IsPathRooted($Value)) { $Value } else { Join-Path $BaseDirectory $Value }
    (Resolve-Path -LiteralPath $candidate).Path
}

function Invoke-PinnedJsonGet(
    [Uri]$Uri,
    [string]$Token,
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$ClientCertificate,
    [string]$ExpectedCertificateSha256,
    [int]$TimeoutSeconds
) {
    $handler = [XhubPinnedHttpClientHandler]::new($ExpectedCertificateSha256)
    [void]$handler.ClientCertificates.Add($ClientCertificate)
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds($TimeoutSeconds)
    try {
        $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get, $Uri)
        [void]$request.Headers.TryAddWithoutValidation("authorization", "Bearer $Token")
        [void]$request.Headers.TryAddWithoutValidation("x-xhub-protocol-version", "0x0360")
        try {
            $response = $client.SendAsync($request).GetAwaiter().GetResult()
            if (-not $response.IsSuccessStatusCode) { throw "Watchtower returned HTTP $([int]$response.StatusCode)" }
            $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
            if ($body.Length -gt 2097152) { throw "Watchtower response exceeded 2 MiB" }
            if (-not $handler.CertificateAccepted) { throw "TLS certificate was not accepted and pinned" }
            [pscustomobject]@{
                json = ($body | ConvertFrom-Json)
                certificate_sha256 = $handler.ObservedCertificateSha256
                http_status = [int]$response.StatusCode
            }
        } finally {
            $request.Dispose()
        }
    } finally {
        $client.Dispose()
        $handler.Dispose()
    }
}

$probePath = (Resolve-Path -LiteralPath $ProbeProfilePath).Path
$probeDirectory = Split-Path $probePath -Parent
$probe = Get-Content -LiteralPath $probePath -Raw | ConvertFrom-Json
$identities = Get-Content -LiteralPath (Resolve-Path -LiteralPath $IdentityManifestPath) -Raw | ConvertFrom-Json
$tlsProfile = Get-Content -LiteralPath (Resolve-Path -LiteralPath $TlsProfilePath) -Raw | ConvertFrom-Json

Assert-ExactFields $probe @(
    "schema", "protocol_version", "funding_coin_id", "expected_state_sequence",
    "expected_checkpoint_hash", "expected_recovery_package_content_hash", "timeout_seconds",
    "nodes", "production_approved", "production_broadcast"
) "endpoint probe profile"
if ($probe.schema -ne "xhub-v3-6-watchtower-endpoint-probe-1" -or $probe.protocol_version -ne "0x0360") {
    throw "Unsupported Watchtower endpoint probe profile"
}
if ($probe.production_approved -ne $false -or $probe.production_broadcast -ne $false) {
    throw "Endpoint probe cannot grant production approval or broadcast"
}
Assert-Hex ([string]$probe.funding_coin_id) 64 "funding_coin_id"
Assert-Hex ([string]$probe.expected_checkpoint_hash) 64 "expected_checkpoint_hash"
Assert-Hex ([string]$probe.expected_recovery_package_content_hash) 64 "expected_recovery_package_content_hash"
if ([int64]$probe.expected_state_sequence -lt 1) { throw "expected_state_sequence must be positive" }
if ([int64]$probe.timeout_seconds -lt 5 -or [int64]$probe.timeout_seconds -gt 60) { throw "timeout_seconds must be in 5..60" }
if ($identities.schema -ne "xhub-v3-6-watchtower-identities-1" -or
    $tlsProfile.schema -ne "xhub-v3-6-watchtower-tls-profile-1" -or
    $identities.production_approved -ne $false -or $tlsProfile.production_approved -ne $false) {
    throw "Identity and TLS inputs must be fail-closed V3.6 candidate profiles"
}

$identityById = @{}
foreach ($entry in @($identities.attesters)) { $identityById[([string]$entry.attester_id).ToLowerInvariant()] = $entry }
$tlsById = @{}
foreach ($entry in @($tlsProfile.nodes)) { $tlsById[([string]$entry.attester_id).ToLowerInvariant()] = $entry }
$nodes = @($probe.nodes)
if ($nodes.Count -ne 3 -or $identityById.Count -ne 3 -or $tlsById.Count -ne 3) { throw "Probe, identity, and TLS profiles must each contain three nodes" }
$nodeFields = @(
    "attester_id", "api_base_url", "tls_certificate_sha256", "api_token_file",
    "client_certificate_pfx_file", "client_certificate_password_file"
)
$ids = @()
$tokenPaths = @()
$pfxPaths = @()
$passwordPaths = @()
foreach ($node in $nodes) {
    Assert-ExactFields $node $nodeFields "endpoint probe node"
    $id = ([string]$node.attester_id).ToLowerInvariant()
    if ([string]::IsNullOrWhiteSpace($id) -or $id -match '^replace_with_' -or
        -not $identityById.ContainsKey($id) -or -not $tlsById.ContainsKey($id)) {
        throw "Probe node does not map to a frozen Watchtower identity: $id"
    }
    $identity = $identityById[$id]
    $tls = $tlsById[$id]
    if ([string]$node.api_base_url -cne [string]$identity.api_base_url -or
        [string]$node.api_base_url -cne [string]$tls.public_base_url -or
        [string]$node.tls_certificate_sha256 -ine [string]$identity.tls_certificate_sha256 -or
        [string]$node.tls_certificate_sha256 -ine [string]$tls.server_certificate_sha256) {
        throw "Probe endpoint or certificate pin differs from identity/TLS profile: $id"
    }
    $uri = $null
    if (-not [Uri]::TryCreate([string]$node.api_base_url, [UriKind]::Absolute, [ref]$uri) -or
        $uri.Scheme -ne "https" -or $uri.IsLoopback -or $uri.HostNameType -ne [UriHostNameType]::Dns) {
        throw "Probe endpoint must be public HTTPS DNS origin"
    }
    Assert-Hex ([string]$node.tls_certificate_sha256) 64 "tls_certificate_sha256"
    foreach ($field in @("api_token_file", "client_certificate_pfx_file", "client_certificate_password_file")) {
        Assert-ExplicitPath ([string]$node.$field) $field
    }
    $ids += $id
    $tokenPaths += ([string]$node.api_token_file).ToLowerInvariant()
    $pfxPaths += ([string]$node.client_certificate_pfx_file).ToLowerInvariant()
    $passwordPaths += ([string]$node.client_certificate_password_file).ToLowerInvariant()
}
if (($ids | Select-Object -Unique).Count -ne 3 -or
    ($tokenPaths | Select-Object -Unique).Count -ne 3 -or
    ($pfxPaths | Select-Object -Unique).Count -ne 3 -or
    ($passwordPaths | Select-Object -Unique).Count -ne 3) {
    throw "Probe identities, API tokens, client certificates, and password files must be distinct"
}

if ($PlanOnly) {
    [pscustomobject]@{
        schema = "xhub-v3-6-watchtower-endpoint-probe-plan-1"
        protocol_version = "0x0360"
        node_count = 3
        tls13_required = $true
        mutual_tls_required = $true
        certificate_pinning_required = $true
        secret_values_disclosed = $false
        live_endpoint_check_performed = $false
        production_approved = $false
        production_broadcast = $false
        status = "TLS_ENDPOINT_PROBE_PLAN_VALIDATED_ONLY"
    } | ConvertTo-Json -Depth 5
    exit 0
}

$observations = @()
foreach ($node in $nodes) {
    $tokenPath = Resolve-ProbePath $probeDirectory ([string]$node.api_token_file)
    $pfxPath = Resolve-ProbePath $probeDirectory ([string]$node.client_certificate_pfx_file)
    $passwordPath = Resolve-ProbePath $probeDirectory ([string]$node.client_certificate_password_file)
    $token = (Get-Content -LiteralPath $tokenPath -Raw).Trim()
    $password = (Get-Content -LiteralPath $passwordPath -Raw).TrimEnd("`r", "`n")
    if ($token.Length -lt 32 -or $token -match '[\r\n]') { throw "API token file is invalid for $($node.attester_id)" }
    if ($password.Length -lt 16 -or $password -match '[\r\n]') { throw "PFX password file is invalid for $($node.attester_id)" }
    $clientCertificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
        $pfxPath,
        $password,
        [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
    )
    try {
        if (-not $clientCertificate.HasPrivateKey) { throw "Client certificate has no private key for $($node.attester_id)" }
        $base = ([string]$node.api_base_url).TrimEnd('/')
        $health = Invoke-PinnedJsonGet ([Uri]("$base/api/v3.6/health")) $token $clientCertificate ([string]$node.tls_certificate_sha256) ([int]$probe.timeout_seconds)
        if ($health.json.protocol_version -ne "0x0360" -or $health.json.service -ne "watchtower" -or $health.json.status -ne "READY") {
            throw "Watchtower health response is invalid for $($node.attester_id)"
        }
        $latestUri = [Uri]("$base/api/v3.6/funding-coins/$($probe.funding_coin_id)/recovery-packages/latest?protocol_version=0x0360")
        $latest = Invoke-PinnedJsonGet $latestUri $token $clientCertificate ([string]$node.tls_certificate_sha256) ([int]$probe.timeout_seconds)
        if ($latest.json.protocol_version -ne "0x0360" -or
            [string]$latest.json.funding_coin_id -ine [string]$probe.funding_coin_id -or
            [int64]$latest.json.state_sequence -ne [int64]$probe.expected_state_sequence -or
            [string]$latest.json.checkpoint_hash -ine [string]$probe.expected_checkpoint_hash -or
            [string]$latest.json.recovery_package_content_hash -ine [string]$probe.expected_recovery_package_content_hash) {
            throw "Watchtower latest RecoveryPackage differs from expected binding for $($node.attester_id)"
        }
        $observations += [ordered]@{
            attester_id = [string]$node.attester_id
            api_host = ([Uri]$node.api_base_url).DnsSafeHost
            certificate_sha256 = $health.certificate_sha256
            health_http_status = $health.http_status
            package_http_status = $latest.http_status
            state_sequence = [int64]$latest.json.state_sequence
            checkpoint_hash = [string]$latest.json.checkpoint_hash
            recovery_package_content_hash = [string]$latest.json.recovery_package_content_hash
            verified = $true
        }
    } finally {
        $clientCertificate.Dispose()
        $token = $null
        $password = $null
    }
}

[pscustomobject]@{
    schema = "xhub-v3-6-watchtower-tls-endpoint-report-1"
    protocol_version = "0x0360"
    funding_coin_id = [string]$probe.funding_coin_id
    state_sequence = [int64]$probe.expected_state_sequence
    checkpoint_hash = [string]$probe.expected_checkpoint_hash
    recovery_package_content_hash = [string]$probe.expected_recovery_package_content_hash
    observations = $observations
    observed_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
    tls13_required = $true
    mutual_tls_used = $true
    certificate_pins_verified = $true
    secret_values_disclosed = $false
    production_approved = $false
    production_broadcast = $false
    status = "TLS_ENDPOINTS_VERIFIED"
} | ConvertTo-Json -Depth 8
