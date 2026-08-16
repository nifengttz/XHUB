$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-deployment-evidence.ps1"

function New-Fixtures {
    $coin = "61" * 32
    $checkpoint = "62" * 32
    $packageHash = "63" * 32
    $identities = @()
    $observations = @()
    for ($index = 0; $index -lt 3; $index++) {
        $suffix = [char]([int][char]'a' + $index)
        $id = "custody-$suffix"
        $identities += [ordered]@{
            attester_id=$id; failure_domain="domain-$suffix"; operator_id="operator-$suffix"
            infrastructure_provider="provider-$suffix"; region="region-$suffix"
            attester_public_key=((@("{0:x2}" -f (0x81 + $index)) * 48) -join "")
            api_base_url="https://wt-$suffix.example.com"
            tls_certificate_sha256=((@("{0:x2}" -f (0x21 + $index)) * 32) -join "")
        }
        $observations += [ordered]@{
            attester_id=$id; api_host="wt-$suffix.example.com"
            certificate_sha256=((@("{0:x2}" -f (0x21 + $index)) * 32) -join "")
            health_http_status=200; package_http_status=200; state_sequence=1
            checkpoint_hash=$checkpoint; recovery_package_content_hash=$packageHash; verified=$true
        }
    }
    $identityManifest = [ordered]@{
        schema="xhub-v3-6-watchtower-identities-1"; protocol_version="0x0360"; network="mainnet"
        merchant_receipt_public_key=("a8" * 48); custody_attestation_threshold=2
        custody_attestation_participants=3; attesters=$identities; production_approved=$false
    }
    $tlsReport = [ordered]@{
        schema="xhub-v3-6-watchtower-tls-endpoint-report-1"; protocol_version="0x0360"
        funding_coin_id=$coin; state_sequence=1; checkpoint_hash=$checkpoint
        recovery_package_content_hash=$packageHash; observations=$observations
        observed_at_utc=[DateTimeOffset]::UtcNow.ToString("o"); tls13_required=$true
        mutual_tls_used=$true; certificate_pins_verified=$true; secret_values_disclosed=$false
        production_approved=$false; production_broadcast=$false; status="TLS_ENDPOINTS_VERIFIED"
    }
    [pscustomobject]@{ Identities=$identityManifest; TlsReport=$tlsReport }
}

function Invoke-EvidenceValidator([scriptblock]$BeforeEvidence, [scriptblock]$AfterEvidence) {
    $fixtures = New-Fixtures
    if ($null -ne $BeforeEvidence) { & $BeforeEvidence $fixtures }
    $identityPath = Join-Path $env:TEMP ("xhub-deployment-identities-" + [guid]::NewGuid() + ".json")
    $tlsPath = Join-Path $env:TEMP ("xhub-deployment-tls-" + [guid]::NewGuid() + ".json")
    $evidencePath = Join-Path $env:TEMP ("xhub-deployment-evidence-" + [guid]::NewGuid() + ".json")
    try {
        $fixtures.Identities | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $identityPath -Encoding utf8
        $fixtures.TlsReport | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $tlsPath -Encoding utf8
        $identityHash = (Get-FileHash -LiteralPath $identityPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $tlsHash = (Get-FileHash -LiteralPath $tlsPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $operatorVerifications = @($fixtures.Identities.attesters | ForEach-Object {
            [ordered]@{
                attester_id=$_.attester_id; operator_id=$_.operator_id; failure_domain=$_.failure_domain
                deployment_id=("deployment-" + $_.attester_id); evidence_reference=("ticket-" + $_.attester_id)
                status="VERIFIED"
            }
        })
        $reviewers = @(
            [ordered]@{reviewer_id="reviewer-a";failure_domain="review-domain-a";decision="APPROVED";reviewed_identity_manifest_sha256=$identityHash;reviewed_tls_endpoint_report_sha256=$tlsHash},
            [ordered]@{reviewer_id="reviewer-b";failure_domain="review-domain-b";decision="APPROVED";reviewed_identity_manifest_sha256=$identityHash;reviewed_tls_endpoint_report_sha256=$tlsHash}
        )
        $evidence = [ordered]@{
            schema="xhub-v3-6-watchtower-deployment-evidence-1"; protocol_version="0x0360"
            identity_manifest_sha256=$identityHash; tls_endpoint_report_sha256=$tlsHash
            funding_coin_id=$fixtures.TlsReport.funding_coin_id; state_sequence=$fixtures.TlsReport.state_sequence
            checkpoint_hash=$fixtures.TlsReport.checkpoint_hash
            recovery_package_content_hash=$fixtures.TlsReport.recovery_package_content_hash
            operator_verifications=$operatorVerifications; reviewers=$reviewers
            production_approved=$false; production_broadcast=$false
        }
        if ($null -ne $AfterEvidence) { & $AfterEvidence $evidence }
        $evidence | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $evidencePath -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator `
            -EvidencePath $evidencePath -IdentityManifestPath $identityPath -TlsEndpointReportPath $tlsPath 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        [pscustomobject]@{ ExitCode=$exitCode; Output=$output }
    } finally {
        Remove-Item -LiteralPath $identityPath,$tlsPath,$evidencePath -Force -ErrorAction SilentlyContinue
    }
}

$valid = Invoke-EvidenceValidator $null $null
if ($valid.ExitCode -ne 0 -or ($valid.Output | ConvertFrom-Json).status -ne "THREE_OPERATORS_VERIFIED") {
    throw "Valid Watchtower deployment evidence was rejected: $($valid.Output)"
}
$beforeMutations = @(
    { param($f) $f.TlsReport.observed_at_utc = [DateTimeOffset]::UtcNow.AddHours(-25).ToString("o") },
    { param($f) $f.TlsReport.observations[0].verified = $false },
    { param($f) $f.TlsReport.certificate_pins_verified = $false },
    { param($f) $f.TlsReport.observations[0].certificate_sha256 = "99" * 32 },
    { param($f) $f.Identities.attesters[1].infrastructure_provider = $f.Identities.attesters[0].infrastructure_provider }
)
foreach ($mutation in $beforeMutations) {
    if ((Invoke-EvidenceValidator $mutation $null).ExitCode -eq 0) { throw "Invalid TLS deployment report was accepted" }
}
$afterMutations = @(
    { param($e) $e.operator_verifications[1].deployment_id = $e.operator_verifications[0].deployment_id },
    { param($e) $e.operator_verifications[1].operator_id = $e.operator_verifications[0].operator_id },
    { param($e) $e.reviewers[1].failure_domain = $e.reviewers[0].failure_domain },
    { param($e) $e.reviewers[1].decision = "REJECTED" },
    { param($e) $e.identity_manifest_sha256 = "00" * 32 },
    { param($e) $e.production_broadcast = $true }
)
foreach ($mutation in $afterMutations) {
    if ((Invoke-EvidenceValidator $null $mutation).ExitCode -eq 0) { throw "Invalid Watchtower deployment evidence was accepted" }
}
Write-Output "WATCHTOWER_DEPLOYMENT_EVIDENCE_TESTS_OK"
