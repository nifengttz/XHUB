$ErrorActionPreference = "Stop"
$generator = Join-Path $PSScriptRoot "generate-candidate-release.ps1"
$validator = Join-Path $PSScriptRoot "verify-candidate-release.ps1"
$directory = Join-Path $env:TEMP ("xhub-mainnet-release-" + [guid]::NewGuid())
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generator -OutputDirectory $directory
    if ($LASTEXITCODE -ne 0) { throw "Candidate release generation failed" }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ReleaseDirectory $directory
    if ($LASTEXITCODE -ne 0) { throw "Candidate release verification failed" }
    Add-Content -LiteralPath (Join-Path $directory "deploy/mainnet/README.md") -Value "tampered"
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ReleaseDirectory $directory *> $null
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($exitCode -eq 0) { throw "Tampered candidate release was accepted" }
} finally {
    Remove-Item -LiteralPath $directory -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "CANDIDATE_RELEASE_TESTS_OK"
