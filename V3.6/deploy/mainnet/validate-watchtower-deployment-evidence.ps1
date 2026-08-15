param(
    [Parameter(Mandatory = $true)][string]$EvidencePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$TlsEndpointReportPath
)

$ErrorActionPreference = "Stop"

function Assert-ExactFields($Object, [string[]]$Expected, [string]$Context) {
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "|") -ne ($expectedSorted -join "|")) { throw "$Context fields do not match the frozen schema" }
}

function Assert-Hex([string]$Value, [int]$Length, [string]$Field) {
    if ($Value -notmatch "^[0-9a-fA-F]{$Length}$") { throw "$Field must be exactly $Length hexadecimal characters" }
}

function Assert-Name([string]$Value, [string]$Field) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value.Length -gt 256 -or
        $Value -match '^REPLACE_WITH_' -or $Value -match '[\x00-\x1f\x7f]') {
        throw "$Field must be an explicit 1..256 character value"
    }
}

$evidenceResolved = (Resolve-Path -LiteralPath $EvidencePath).Path
$identityResolved = (Resolve-Path -LiteralPath $IdentityManifestPath).Path
$tlsReportResolved = (Resolve-Path -LiteralPath $TlsEndpointReportPath).Path
$evidence = Get-Content -LiteralPath $evidenceResolved -Raw | ConvertFrom-Json
$identities = Get-Content -LiteralPath $identityResolved -Raw | ConvertFrom-Json
$tlsReport = Get-Content -LiteralPath $tlsReportResolved -Raw | ConvertFrom-Json

Assert-ExactFields $evidence @(
    "schema", "protocol_version", "identity_manifest_sha256", "tls_endpoint_report_sha256",
    "funding_coin_id", "state_sequence", "checkpoint_hash", "recovery_package_content_hash",
    "operator_verifications", "reviewers", "production_approved", "production_broadcast"
) "deployment evidence"
if ($evidence.schema -ne "xhub-v3-6-watchtower-deployment-evidence-1" -or $evidence.protocol_version -ne "0x0360") {
    throw "Unsupported Watchtower deployment evidence"
}
if ($evidence.production_approved -ne $false -or $evidence.production_broadcast -ne $false) {
    throw "Deployment evidence cannot grant production approval or broadcast"
}
$identityHash = (Get-FileHash -LiteralPath $identityResolved -Algorithm SHA256).Hash.ToLowerInvariant()
$tlsReportHash = (Get-FileHash -LiteralPath $tlsReportResolved -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$evidence.identity_manifest_sha256 -ine $identityHash -or
    [string]$evidence.tls_endpoint_report_sha256 -ine $tlsReportHash) {
    throw "Deployment evidence input hashes do not match"
}
if ($identities.schema -ne "xhub-v3-6-watchtower-identities-1" -or
    $identities.network -ne "mainnet" -or $identities.production_approved -ne $false) {
    throw "Identity manifest is not a fail-closed mainnet candidate"
}
if ($tlsReport.schema -ne "xhub-v3-6-watchtower-tls-endpoint-report-1" -or
    $tlsReport.status -ne "TLS_ENDPOINTS_VERIFIED" -or
    $tlsReport.production_approved -ne $false -or $tlsReport.production_broadcast -ne $false -or
    $tlsReport.tls13_required -ne $true -or $tlsReport.mutual_tls_used -ne $true -or
    $tlsReport.certificate_pins_verified -ne $true -or $tlsReport.secret_values_disclosed -ne $false) {
    throw "TLS endpoint report is not a verified fail-closed report"
}
$observedAt = [DateTimeOffset]::MinValue
if (-not [DateTimeOffset]::TryParse([string]$tlsReport.observed_at_utc, [ref]$observedAt) -or
    $observedAt.Offset -ne [TimeSpan]::Zero -or $observedAt -gt [DateTimeOffset]::UtcNow.AddMinutes(5) -or
    $observedAt -lt [DateTimeOffset]::UtcNow.AddHours(-24)) {
    throw "TLS endpoint report must be a UTC observation from the last 24 hours"
}
foreach ($field in @("funding_coin_id", "checkpoint_hash", "recovery_package_content_hash")) {
    Assert-Hex ([string]$evidence.$field) 64 $field
    if ([string]$evidence.$field -ine [string]$tlsReport.$field) { throw "Deployment evidence differs from TLS report: $field" }
}
if ([int64]$evidence.state_sequence -lt 1 -or [int64]$evidence.state_sequence -ne [int64]$tlsReport.state_sequence) {
    throw "Deployment evidence state_sequence differs from TLS report"
}

$identityById = @{}
foreach ($identity in @($identities.attesters)) { $identityById[([string]$identity.attester_id).ToLowerInvariant()] = $identity }
foreach ($field in @("operator_id", "failure_domain", "infrastructure_provider", "region", "attester_public_key", "api_base_url", "tls_certificate_sha256")) {
    $values = @($identities.attesters | ForEach-Object { ([string]$_.$field).ToLowerInvariant() })
    if ($values.Count -ne 3 -or ($values | Select-Object -Unique).Count -ne 3) {
        throw "Identity manifest does not contain three independent values for $field"
    }
}
$observationById = @{}
foreach ($observation in @($tlsReport.observations)) {
    $key = ([string]$observation.attester_id).ToLowerInvariant()
    if ($observation.verified -ne $true -or $observationById.ContainsKey($key) -or -not $identityById.ContainsKey($key)) {
        throw "TLS observations must be unique, verified, and identity-bound"
    }
    $identity = $identityById[$key]
    if ([string]$observation.api_host -ine ([Uri]$identity.api_base_url).DnsSafeHost -or
        [string]$observation.certificate_sha256 -ine [string]$identity.tls_certificate_sha256 -or
        [int64]$observation.health_http_status -ne 200 -or [int64]$observation.package_http_status -ne 200) {
        throw "TLS observation endpoint or certificate differs from identity manifest: $key"
    }
    if ([string]$observation.recovery_package_content_hash -ine [string]$evidence.recovery_package_content_hash -or
        [string]$observation.checkpoint_hash -ine [string]$evidence.checkpoint_hash -or
        [int64]$observation.state_sequence -ne [int64]$evidence.state_sequence) {
        throw "TLS observation differs from deployment binding: $key"
    }
    $observationById[$key] = $observation
}
if ($identityById.Count -ne 3 -or $observationById.Count -ne 3) { throw "Exactly three identity and TLS observations are required" }

$verificationFields = @("attester_id", "operator_id", "failure_domain", "deployment_id", "evidence_reference", "status")
$verifiedIds = @()
$deploymentIds = @()
foreach ($verification in @($evidence.operator_verifications)) {
    Assert-ExactFields $verification $verificationFields "operator verification"
    foreach ($field in @("attester_id", "operator_id", "failure_domain", "deployment_id", "evidence_reference")) {
        Assert-Name ([string]$verification.$field) $field
    }
    if ($verification.status -ne "VERIFIED") { throw "Each operator deployment must be VERIFIED" }
    $key = ([string]$verification.attester_id).ToLowerInvariant()
    if (-not $identityById.ContainsKey($key) -or -not $observationById.ContainsKey($key)) {
        throw "Operator verification has no matching identity and TLS observation: $key"
    }
    $identity = $identityById[$key]
    if ([string]$verification.operator_id -cne [string]$identity.operator_id -or
        [string]$verification.failure_domain -cne [string]$identity.failure_domain) {
        throw "Operator verification differs from identity manifest: $key"
    }
    $verifiedIds += $key
    $deploymentIds += ([string]$verification.deployment_id).ToLowerInvariant()
}
if ($verifiedIds.Count -ne 3 -or ($verifiedIds | Select-Object -Unique).Count -ne 3 -or
    ($deploymentIds | Select-Object -Unique).Count -ne 3) {
    throw "Three unique operator and deployment IDs are required"
}

$reviewerFields = @(
    "reviewer_id", "failure_domain", "decision", "reviewed_identity_manifest_sha256",
    "reviewed_tls_endpoint_report_sha256"
)
$reviewers = @($evidence.reviewers)
if ($reviewers.Count -ne 2) { throw "Exactly two deployment reviewers are required" }
foreach ($reviewer in $reviewers) {
    Assert-ExactFields $reviewer $reviewerFields "deployment reviewer"
    Assert-Name ([string]$reviewer.reviewer_id) "reviewer_id"
    Assert-Name ([string]$reviewer.failure_domain) "reviewer failure_domain"
    if ($reviewer.decision -ne "APPROVED" -or
        [string]$reviewer.reviewed_identity_manifest_sha256 -ine $identityHash -or
        [string]$reviewer.reviewed_tls_endpoint_report_sha256 -ine $tlsReportHash) {
        throw "Deployment reviewer did not approve the exact identity and TLS reports"
    }
}
if (($reviewers.reviewer_id | ForEach-Object { ([string]$_).ToLowerInvariant() } | Select-Object -Unique).Count -ne 2 -or
    ($reviewers.failure_domain | ForEach-Object { ([string]$_).ToLowerInvariant() } | Select-Object -Unique).Count -ne 2) {
    throw "Deployment reviewers and reviewer failure domains must be distinct"
}

[pscustomobject]@{
    schema = "xhub-v3-6-watchtower-deployment-validation-1"
    protocol_version = "0x0360"
    identity_manifest_sha256 = $identityHash
    tls_endpoint_report_sha256 = $tlsReportHash
    operator_count = 3
    operator_failure_domain_count = 3
    endpoint_count = 3
    reviewer_count = 2
    production_approved = $false
    production_broadcast = $false
    status = "THREE_OPERATORS_VERIFIED"
} | ConvertTo-Json -Depth 5
