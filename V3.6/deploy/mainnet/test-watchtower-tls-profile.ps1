$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-tls-profile.ps1"

function New-IdentityManifest {
    $entries = @()
    for ($index = 0; $index -lt 3; $index++) {
        $suffix = [char]([int][char]'a' + $index)
        $entries += [ordered]@{
            attester_id = "custody-$suffix"
            failure_domain = "failure-$suffix"
            operator_id = "operator-$suffix"
            infrastructure_provider = "provider-$suffix"
            region = "region-$suffix"
            attester_public_key = ((@("{0:x2}" -f (0x81 + $index)) * 48) -join "")
            api_base_url = "https://wt-$suffix.example.com"
            tls_certificate_sha256 = ((@("{0:x2}" -f (0x11 + $index)) * 32) -join "")
        }
    }
    [ordered]@{
        schema = "xhub-v3-6-watchtower-identities-1"
        protocol_version = "0x0360"
        network = "mainnet"
        merchant_receipt_public_key = ("a8" * 48)
        custody_attestation_threshold = 2
        custody_attestation_participants = 3
        attesters = $entries
        production_approved = $false
    }
}

function New-TlsProfile($identities) {
    $nodes = @()
    for ($index = 0; $index -lt 3; $index++) {
        $identity = $identities.attesters[$index]
        $nodes += [ordered]@{
            attester_id = $identity.attester_id
            public_base_url = $identity.api_base_url
            upstream_url = "http://127.0.0.1:8738"
            server_certificate_file = "C:/xhub/node-$index/server.crt"
            server_private_key_file = "C:/xhub/node-$index/server.key"
            trusted_client_ca_file = "C:/xhub/node-$index/client-ca.crt"
            server_certificate_sha256 = $identity.tls_certificate_sha256
            minimum_tls_version = "TLS1.3"
            client_certificate_mode = "require_and_verify"
            hsts_max_age_seconds = 31536000
            rate_limit_requests_per_minute = 120
            max_request_body_bytes = 1048576
        }
    }
    [ordered]@{
        schema = "xhub-v3-6-watchtower-tls-profile-1"
        protocol_version = "0x0360"
        nodes = $nodes
        production_approved = $false
    }
}

function Invoke-TlsValidator($profile, $identities) {
    $profilePath = Join-Path $env:TEMP ("xhub-tls-" + [guid]::NewGuid() + ".json")
    $identityPath = Join-Path $env:TEMP ("xhub-tls-identities-" + [guid]::NewGuid() + ".json")
    try {
        $profile | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $profilePath -Encoding utf8
        $identities | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $identityPath -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -TlsProfilePath $profilePath -IdentityManifestPath $identityPath 2>$null
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        [pscustomobject]@{ ExitCode=$exitCode; Output=$output }
    } finally {
        Remove-Item -LiteralPath $profilePath,$identityPath -Force -ErrorAction SilentlyContinue
    }
}

$identities = New-IdentityManifest
$valid = Invoke-TlsValidator (New-TlsProfile $identities) $identities
if ($valid.ExitCode -ne 0 -or ($valid.Output | ConvertFrom-Json).status -ne "TLS_PROFILE_VALIDATED_CONFIG_ONLY") {
    throw "Valid Watchtower TLS profile was rejected"
}
$mutations = @(
    { param($p) $p.nodes[0].public_base_url = "http://wt-a.example.com" },
    { param($p) $p.nodes[0].upstream_url = "http://10.0.0.4:8738" },
    { param($p) $p.nodes[0].minimum_tls_version = "TLS1.2" },
    { param($p) $p.nodes[0].client_certificate_mode = "optional" },
    { param($p) $p.nodes[0].server_certificate_sha256 = ("99" * 32) },
    { param($p) $p.production_approved = $true }
)
foreach ($mutation in $mutations) {
    $profile = New-TlsProfile $identities
    & $mutation $profile
    if ((Invoke-TlsValidator $profile $identities).ExitCode -eq 0) { throw "Invalid Watchtower TLS profile was accepted" }
}
$selfApprovedIdentities = New-IdentityManifest
$selfApprovedIdentities.production_approved = $true
if ((Invoke-TlsValidator (New-TlsProfile $selfApprovedIdentities) $selfApprovedIdentities).ExitCode -eq 0) {
    throw "TLS validator accepted a self-approved identity manifest"
}
Write-Output "WATCHTOWER_TLS_PROFILE_TESTS_OK"
