$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-identities.ps1"

$merchant = "a870ee2a2452db2324e2caf2f3f576edc7923d76630b2aa7f259934173c4031ebaab984681de083f2cdd05dbc5807910"
$keys = @(
    "89d0608036649d3484b7cfe71cfbd7f13015081d6206aede1aed0a4c1ad1521233123c08f0870e9d9f605ed429d24419",
    "b61c4ee5d1cdd57ea615e6f3003e89afeee153d666562d0abec363d8b88c21c35e55f5622668b113e966564d04eb9fa1",
    "97b418875a833bded791b93b592d6cbff91d9a51cc1fe375e9eb44e379751a48f974ead8948542d31b066229aca6fca5"
)

function New-Manifest {
    $entries = @()
    for ($index = 0; $index -lt 3; $index++) {
        $suffix = [char]([int][char]'a' + $index)
        $entries += [ordered]@{
            attester_id = "custody-$suffix"
            failure_domain = "failure-$suffix"
            operator_id = "operator-$suffix"
            infrastructure_provider = "provider-$suffix"
            region = "region-$suffix"
            attester_public_key = $keys[$index]
            api_base_url = "https://wt-$suffix.example.com"
            tls_certificate_sha256 = ((@("{0:x2}" -f (0x11 + $index)) * 32) -join "")
        }
    }
    [ordered]@{
        schema = "xhub-v3-6-watchtower-identities-1"
        protocol_version = "0x0360"
        network = "mainnet"
        merchant_receipt_public_key = $merchant
        custody_attestation_threshold = 2
        custody_attestation_participants = 3
        attesters = $entries
        production_approved = $false
    }
}

function Invoke-IdentityValidator($manifest) {
    $manifestPath = Join-Path $env:TEMP ("xhub-identities-" + [guid]::NewGuid() + ".json")
    $configPath = Join-Path $env:TEMP ("xhub-attesters-" + [guid]::NewGuid() + ".json")
    try {
        $config = @($manifest.attesters | ForEach-Object {
            [ordered]@{ signer_id=$_.attester_id; failure_domain=$_.failure_domain; signer_public_key=$_.attester_public_key }
        })
        $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
        $config | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -IdentityManifestPath $manifestPath -CustodyAttestersPath $configPath 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        [pscustomobject]@{ ExitCode=$exitCode; Output=$output }
    } finally {
        Remove-Item -LiteralPath $manifestPath,$configPath -Force -ErrorAction SilentlyContinue
    }
}

$valid = Invoke-IdentityValidator (New-Manifest)
if ($valid.ExitCode -ne 0 -or ($valid.Output | ConvertFrom-Json).status -ne "IDENTITIES_VERIFIED_CANDIDATE_ONLY") {
    throw "Valid Watchtower identities were rejected: $($valid.Output)"
}
$mutations = @(
    { param($m) $m.attesters[1].attester_public_key = $m.attesters[0].attester_public_key },
    { param($m) $m.attesters[1].failure_domain = $m.attesters[0].failure_domain },
    { param($m) $m.attesters[1].infrastructure_provider = $m.attesters[0].infrastructure_provider },
    { param($m) $m.attesters[1].api_base_url = "http://wt-b.example.com" },
    { param($m) $m.production_approved = $true }
)
foreach ($mutation in $mutations) {
    $manifest = New-Manifest
    & $mutation $manifest
    if ((Invoke-IdentityValidator $manifest).ExitCode -eq 0) { throw "Invalid Watchtower identities were accepted" }
}
Write-Output "WATCHTOWER_IDENTITY_TESTS_OK"
