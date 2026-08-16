param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "candidate-release")
)

$ErrorActionPreference = "Stop"
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
if (Test-Path -LiteralPath $OutputDirectory) { throw "Candidate release directory already exists: $OutputDirectory" }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$files = @(
    "deploy/mainnet/README.md",
    "deploy/mainnet/config.mainnet.example.json",
    "deploy/mainnet/mainnet-parameters.candidate.json",
    "deploy/mainnet/validate-mainnet-parameters.ps1",
    "deploy/mainnet/validate-config.ps1",
    "deploy/mainnet/rpc-preflight.ps1",
    "deploy/mainnet/canary-plan.example.json",
    "deploy/mainnet/validate-canary-plan.ps1",
    "deploy/mainnet/canary-evidence.example.json",
    "deploy/mainnet/validate-canary-evidence.ps1",
    "deploy/mainnet/mainnet-canary-artifacts.json",
    "deploy/mainnet/verify-artifact-manifest.ps1",
    "deploy/mainnet/check-secret-isolation.ps1",
    "deploy/mainnet/readiness-input.example.json",
    "deploy/mainnet/evaluate-readiness.ps1",
    "deploy/mainnet/validate-rpc-preflight-output.ps1",
    "deploy/mainnet/validate-funding-binding.ps1",
    "deploy/mainnet/validate-recovery-simulation.ps1",
    "deploy/mainnet/approval-records.example.json",
    "deploy/mainnet/validate-approval-records.ps1",
    "deploy/mainnet/run-canary-dry-run.ps1",
    "deploy/mainnet/watchtower-identities.mainnet.example.json",
    "deploy/mainnet/custody-attesters.mainnet.example.json",
    "deploy/mainnet/validate-watchtower-identities.ps1",
    "deploy/mainnet/watchtower-tls-profile.mainnet.example.json",
    "deploy/mainnet/validate-watchtower-tls-profile.ps1",
    "deploy/mainnet/watchtower-endpoint-probe.mainnet.example.json",
    "deploy/mainnet/verify-watchtower-tls-endpoints.ps1",
    "deploy/mainnet/watchtower-deployment-evidence.mainnet.example.json",
    "deploy/mainnet/validate-watchtower-deployment-evidence.ps1",
    "deploy/mainnet/watchtower-runtime.mainnet.example.json",
    "deploy/mainnet/validate-watchtower-runtime.ps1",
    "deploy/mainnet/generate-watchtower-systemd-units.ps1",
    "deploy/mainnet/generate-watchtower-nginx-configs.ps1",
    "deploy/mainnet/verify-watchtower-generated-configs.ps1",
    ".dockerignore",
    "deploy/mainnet/docker-single-vps/Dockerfile",
    "deploy/mainnet/docker-single-vps/README.md",
    "deploy/mainnet/docker-single-vps/single-vps-docker-profile.example.json",
    "deploy/mainnet/docker-single-vps/custody-attesters.single-vps.example.json",
    "deploy/mainnet/docker-single-vps/validate-single-vps-docker-profile.ps1",
    "deploy/mainnet/docker-single-vps/generate-single-vps-docker-compose.ps1",
    "puzzles-v3_6/module-hashes.json"
)
$manifestFiles = @()
foreach ($relative in $files) {
    $source = Join-Path $root $relative
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Candidate release input missing: $relative" }
    $normalized = $relative.Replace("\", "/")
    if ($normalized -match '(?i)(\.local\.|(^|/)(secrets?|private)(/|$)|mnemonic|spendbundle|\.sqlite|\.db)') {
        throw "Candidate release contains prohibited path: $relative"
    }
    $destination = Join-Path $OutputDirectory $relative
    New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination
    $manifestFiles += [ordered]@{
        path = $normalized
        sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifest = [ordered]@{
    schema = "xhub-v3-6-mainnet-canary-candidate-release-1"
    protocol_version = "0x0360"
    release_status = "CANDIDATE_NOT_APPROVED"
    mainnet_approved = $false
    production_broadcast = $false
    contains_secrets = $false
    contains_spend_bundle = $false
    files = $manifestFiles
}
$manifestPath = Join-Path $OutputDirectory "candidate-release-manifest.json"
[System.IO.File]::WriteAllText($manifestPath, (($manifest | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
Write-Output "Generated $OutputDirectory"
