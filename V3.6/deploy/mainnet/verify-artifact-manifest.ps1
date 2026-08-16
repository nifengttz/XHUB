param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot "mainnet-canary-artifacts.json")
)

$ErrorActionPreference = "Stop"
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$manifest = Get-Content -LiteralPath (Resolve-Path -LiteralPath $ManifestPath) -Raw | ConvertFrom-Json
if ($manifest.schema -ne "xhub-v3-6-mainnet-canary-artifacts-1" -or $manifest.protocol_version -ne "0x0360") {
    throw "Unsupported mainnet artifact manifest"
}
if ($manifest.production_broadcast -ne $false) { throw "Artifact manifest cannot authorize production broadcast" }
$seen = @{}
foreach ($artifact in @($manifest.artifacts)) {
    $relative = [string]$artifact.path
    if ([System.IO.Path]::IsPathRooted($relative) -or $relative.Contains("..")) { throw "Unsafe artifact path: $relative" }
    if ($seen.ContainsKey($relative)) { throw "Duplicate artifact path: $relative" }
    $seen[$relative] = $true
    $path = Join-Path $root $relative
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Artifact missing: $relative" }
    $item = Get-Item -LiteralPath $path
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([int64]$artifact.size -ne [int64]$item.Length -or [string]$artifact.sha256 -ne $hash) {
        throw "Artifact hash or size mismatch: $relative"
    }
}
if ($seen.Count -lt 12) { throw "Artifact manifest is incomplete" }
[pscustomobject]@{
    schema = $manifest.schema
    artifact_count = $seen.Count
    production_broadcast = $false
    status = "ARTIFACTS_VERIFIED"
} | ConvertTo-Json -Depth 4
