$ErrorActionPreference = "Stop"
$generator = Join-Path $PSScriptRoot "generate-artifact-manifest.ps1"
$validator = Join-Path $PSScriptRoot "verify-artifact-manifest.ps1"
$manifestPath = Join-Path $PSScriptRoot "mainnet-canary-artifacts.json"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generator -OutputPath $manifestPath
if ($LASTEXITCODE -ne 0) { throw "Artifact manifest generation failed" }
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ManifestPath $manifestPath
if ($LASTEXITCODE -ne 0) { throw "Artifact manifest verification failed" }

$tampered = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$tampered.artifacts[0].sha256 = "00" * 32
$temp = Join-Path $env:TEMP ("xhub-artifacts-" + [guid]::NewGuid() + ".json")
try {
    $tampered | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temp -Encoding utf8
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ManifestPath $temp *> $null
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($exitCode -eq 0) { throw "Tampered artifact manifest was accepted" }
} finally {
    Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue
}
Write-Output "ARTIFACT_MANIFEST_TESTS_OK"
