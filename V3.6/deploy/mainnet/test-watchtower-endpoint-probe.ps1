$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "verify-watchtower-tls-endpoints.ps1"

function New-Fixtures {
    $identities = @()
    $tlsNodes = @()
    $probeNodes = @()
    for ($index = 0; $index -lt 3; $index++) {
        $suffix = [char]([int][char]'a' + $index)
        $id = "custody-$suffix"
        $url = "https://wt-$suffix.example.com"
        $pin = ((@("{0:x2}" -f (0x21 + $index)) * 32) -join "")
        $identities += [ordered]@{
            attester_id=$id; failure_domain="domain-$suffix"; operator_id="operator-$suffix"
            infrastructure_provider="provider-$suffix"; region="region-$suffix"
            attester_public_key=((@("{0:x2}" -f (0x81 + $index)) * 48) -join "")
            api_base_url=$url; tls_certificate_sha256=$pin
        }
        $tlsNodes += [ordered]@{
            attester_id=$id; public_base_url=$url; upstream_url="http://127.0.0.1:8738"
            server_certificate_file="C:/node-$index/server.crt"; server_private_key_file="C:/node-$index/server.key"
            trusted_client_ca_file="C:/node-$index/client-ca.crt"; server_certificate_sha256=$pin
            minimum_tls_version="TLS1.3"; client_certificate_mode="require_and_verify"
            hsts_max_age_seconds=31536000; rate_limit_requests_per_minute=120; max_request_body_bytes=1048576
        }
        $probeNodes += [ordered]@{
            attester_id=$id; api_base_url=$url; tls_certificate_sha256=$pin
            api_token_file="C:/client/node-$index/token.txt"
            client_certificate_pfx_file="C:/client/node-$index/client.pfx"
            client_certificate_password_file="C:/client/node-$index/client-password.txt"
        }
    }
    $identityManifest = [ordered]@{
        schema="xhub-v3-6-watchtower-identities-1"; protocol_version="0x0360"; network="mainnet"
        merchant_receipt_public_key=("a8" * 48); custody_attestation_threshold=2
        custody_attestation_participants=3; attesters=$identities; production_approved=$false
    }
    $tlsProfile = [ordered]@{
        schema="xhub-v3-6-watchtower-tls-profile-1"; protocol_version="0x0360"
        nodes=$tlsNodes; production_approved=$false
    }
    $probe = [ordered]@{
        schema="xhub-v3-6-watchtower-endpoint-probe-1"; protocol_version="0x0360"
        funding_coin_id=("61" * 32); expected_state_sequence=1; expected_checkpoint_hash=("62" * 32)
        expected_recovery_package_content_hash=("63" * 32); timeout_seconds=20; nodes=$probeNodes
        production_approved=$false; production_broadcast=$false
    }
    [pscustomobject]@{ Probe=$probe; Identities=$identityManifest; Tls=$tlsProfile }
}

function Invoke-PlanValidator($fixtures) {
    $probePath = Join-Path $env:TEMP ("xhub-probe-" + [guid]::NewGuid() + ".json")
    $identityPath = Join-Path $env:TEMP ("xhub-probe-identities-" + [guid]::NewGuid() + ".json")
    $tlsPath = Join-Path $env:TEMP ("xhub-probe-tls-" + [guid]::NewGuid() + ".json")
    try {
        $fixtures.Probe | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $probePath -Encoding utf8
        $fixtures.Identities | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $identityPath -Encoding utf8
        $fixtures.Tls | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $tlsPath -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator `
            -ProbeProfilePath $probePath -IdentityManifestPath $identityPath -TlsProfilePath $tlsPath -PlanOnly 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        [pscustomobject]@{ ExitCode=$exitCode; Output=$output }
    } finally {
        Remove-Item -LiteralPath $probePath,$identityPath,$tlsPath -Force -ErrorAction SilentlyContinue
    }
}

$valid = Invoke-PlanValidator (New-Fixtures)
if ($valid.ExitCode -ne 0 -or ($valid.Output | ConvertFrom-Json).status -ne "TLS_ENDPOINT_PROBE_PLAN_VALIDATED_ONLY") {
    throw "Valid TLS endpoint probe plan was rejected: $($valid.Output)"
}
$mutations = @(
    { param($f) $f.Probe.nodes[0].api_base_url = "https://other.example.com" },
    { param($f) $f.Probe.nodes[0].tls_certificate_sha256 = ("99" * 32) },
    { param($f) $f.Probe.nodes[1].api_token_file = $f.Probe.nodes[0].api_token_file },
    { param($f) $f.Probe.nodes[1].client_certificate_pfx_file = $f.Probe.nodes[0].client_certificate_pfx_file },
    { param($f) $f.Probe.nodes[0].client_certificate_pfx_file = "REPLACE_WITH_PFX" },
    { param($f) $f.Probe.expected_checkpoint_hash = "00" },
    { param($f) $f.Probe.production_broadcast = $true }
)
foreach ($mutation in $mutations) {
    $fixtures = New-Fixtures
    & $mutation $fixtures
    if ((Invoke-PlanValidator $fixtures).ExitCode -eq 0) { throw "Invalid TLS endpoint probe plan was accepted" }
}
Write-Output "WATCHTOWER_ENDPOINT_PROBE_TESTS_OK"
