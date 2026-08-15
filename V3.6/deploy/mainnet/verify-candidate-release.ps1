param(
    [Parameter(Mandatory = $true)][string]$ReleaseDirectory
)

$ErrorActionPreference = "Stop"
$directory = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$manifestPath = Join-Path $directory "candidate-release-manifest.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schema -ne "xhub-v3-6-mainnet-canary-candidate-release-1" -or
    $manifest.release_status -ne "CANDIDATE_NOT_APPROVED" -or
    $manifest.mainnet_approved -ne $false -or
    $manifest.production_broadcast -ne $false -or
    $manifest.contains_secrets -ne $false -or
    $manifest.contains_spend_bundle -ne $false) {
    throw "Candidate release safety flags are invalid"
}
$declared = @{}
foreach ($file in @($manifest.files)) {
    $relative = [string]$file.path
    if ([System.IO.Path]::IsPathRooted($relative) -or $relative.Contains("..") -or
        $relative -match '(?i)(\.local\.|(^|/)(secrets?|private)(/|$)|mnemonic|spendbundle|\.sqlite|\.db)') {
        throw "Unsafe candidate release path: $relative"
    }
    if ($declared.ContainsKey($relative)) { throw "Duplicate release path: $relative" }
    $declared[$relative] = $true
    $path = Join-Path $directory $relative
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Release file missing: $relative" }
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne [string]$file.sha256) {
        throw "Release file hash mismatch: $relative"
    }
}
$actualFiles = @(Get-ChildItem -LiteralPath $directory -File -Recurse | Where-Object { $_.FullName -ne $manifestPath })
if ($actualFiles.Count -ne $declared.Count) { throw "Candidate release contains undeclared files" }
[pscustomobject]@{
    schema = $manifest.schema
    file_count = $declared.Count
    mainnet_approved = $false
    production_broadcast = $false
    status = "CANDIDATE_VERIFIED_NOT_APPROVED"
} | ConvertTo-Json -Depth 4
