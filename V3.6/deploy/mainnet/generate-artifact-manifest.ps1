param(
    [string]$OutputPath = (Join-Path $PSScriptRoot "mainnet-canary-artifacts.json")
)

$ErrorActionPreference = "Stop"
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$relativePaths = @(
    "protocol-v3_6/protocol-v3_6.md",
    "protocol-v3_6/test-vectors/protocol-v3_6.json",
    "puzzles-v3_6/module-hashes.json",
    "deploy/mainnet/mainnet-parameters.candidate.json",
    "deploy/mainnet/validate-mainnet-parameters.ps1",
    "deploy/mainnet/config.mainnet.example.json",
    "deploy/mainnet/validate-config.ps1",
    "deploy/mainnet/rpc-preflight.ps1",
    "deploy/mainnet/canary-plan.example.json",
    "deploy/mainnet/validate-canary-plan.ps1",
    "deploy/mainnet/canary-evidence.example.json",
    "deploy/mainnet/validate-canary-evidence.ps1",
    "deploy/mainnet/validate-rpc-preflight-output.ps1",
    "deploy/mainnet/validate-funding-binding.ps1",
    "deploy/mainnet/validate-recovery-simulation.ps1",
    "deploy/mainnet/approval-records.example.json",
    "deploy/mainnet/validate-approval-records.ps1",
    "deploy/mainnet/run-canary-dry-run.ps1"
    "deploy/mainnet/watchtower-identities.mainnet.example.json"
    "deploy/mainnet/custody-attesters.mainnet.example.json"
    "deploy/mainnet/validate-watchtower-identities.ps1"
    "deploy/mainnet/watchtower-tls-profile.mainnet.example.json"
    "deploy/mainnet/validate-watchtower-tls-profile.ps1"
    "deploy/mainnet/watchtower-endpoint-probe.mainnet.example.json"
    "deploy/mainnet/verify-watchtower-tls-endpoints.ps1"
    "deploy/mainnet/watchtower-deployment-evidence.mainnet.example.json"
    "deploy/mainnet/validate-watchtower-deployment-evidence.ps1"
    "deploy/mainnet/watchtower-runtime.mainnet.example.json"
    "deploy/mainnet/validate-watchtower-runtime.ps1"
    "deploy/mainnet/generate-watchtower-systemd-units.ps1"
    "deploy/mainnet/generate-watchtower-nginx-configs.ps1"
    "deploy/mainnet/verify-watchtower-generated-configs.ps1"
    ".dockerignore"
    "deploy/mainnet/docker-single-vps/Dockerfile"
    "deploy/mainnet/docker-single-vps/README.md"
    "deploy/mainnet/docker-single-vps/single-vps-docker-profile.example.json"
    "deploy/mainnet/docker-single-vps/custody-attesters.single-vps.example.json"
    "deploy/mainnet/docker-single-vps/validate-single-vps-docker-profile.ps1"
    "deploy/mainnet/docker-single-vps/generate-single-vps-docker-compose.ps1"
)
$artifacts = @()
foreach ($relativePath in $relativePaths) {
    $path = Join-Path $root $relativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing canary artifact: $relativePath" }
    $item = Get-Item -LiteralPath $path
    $artifacts += [ordered]@{
        path = $relativePath.Replace("\", "/")
        size = [int64]$item.Length
        sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifest = [ordered]@{
    schema = "xhub-v3-6-mainnet-canary-artifacts-1"
    protocol_version = "0x0360"
    production_broadcast = $false
    artifacts = $artifacts
}
[System.IO.File]::WriteAllText($OutputPath, (($manifest | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
Write-Output "Generated $OutputPath"
