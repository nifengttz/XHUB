param(
    [string]$RepositoryRoot = (Split-Path $PSScriptRoot -Parent),
    [string]$OutputPath = (Join-Path $PSScriptRoot "source-control-candidate-manifest.json"),
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$audit = Join-Path $PSScriptRoot "audit-source-control-readiness.ps1"
$auditResult = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $audit -RepositoryRoot $repoRoot | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $auditResult.status -ne "READY_FOR_MANUAL_GIT_REVIEW") {
    throw "Source-control readiness audit did not pass"
}

$outputFullPath = [IO.Path]::GetFullPath($OutputPath)
if ((Test-Path -LiteralPath $outputFullPath) -and -not $Force) {
    throw "Output already exists: $outputFullPath"
}
$excluded = @(
    "V3.6/source-control-readiness-evidence.json",
    "V3.6/source-control-candidate-manifest.json",
    "V3.6/source-control-candidate-manifest-verification-evidence.json"
)
$candidateOutput = & git -C $repoRoot -c core.quotepath=false ls-files --cached --others --exclude-standard -- V3.6
if ($LASTEXITCODE -ne 0) { throw "git ls-files failed" }
$candidates = @($candidateOutput |
    Where-Object { $_ -and $_.Trim() } |
    ForEach-Object { $_.Replace('\', '/') } |
    Where-Object { $excluded -notcontains $_ } |
    Sort-Object)
if ($candidates.Count -ne [int]$auditResult.candidate_file_count) {
    throw "Candidate count differs from readiness audit"
}

$entries = @()
$totalBytes = 0L
foreach ($candidate in $candidates) {
    $absolute = Join-Path $repoRoot $candidate
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) {
        throw "Candidate file is missing: $candidate"
    }
    $size = (Get-Item -LiteralPath $absolute).Length
    $sha256 = (Get-FileHash -LiteralPath $absolute -Algorithm SHA256).Hash.ToLowerInvariant()
    $entries += [ordered]@{ path = $candidate; size = $size; sha256 = $sha256 }
    $totalBytes += $size
}

$treeMaterial = ($entries | ForEach-Object { "$($_.path)`0$($_.sha256)`0$($_.size)`n" }) -join ''
$hasher = [Security.Cryptography.SHA256]::Create()
try {
    $treeHashBytes = $hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($treeMaterial))
} finally {
    $hasher.Dispose()
}
$treeSha256 = -join ($treeHashBytes | ForEach-Object { $_.ToString('x2') })

$manifest = [ordered]@{
    schema = "xhub-v3-6-source-control-candidate-manifest-1"
    protocol_version = "0x0360"
    candidate_root = "V3.6"
    candidate_file_count = $entries.Count
    candidate_total_bytes = $totalBytes
    tree_sha256 = $treeSha256
    excluded_reports = $excluded
    files = $entries
    private_payloads_included = $false
    local_secrets_included = $false
    generated_sqlite_included = $false
    commit_created = $false
    automatic_staging_enabled = $false
    manual_review_required = $true
    production_ready = $false
    spend_bundle_created = $false
    broadcast_enabled = $false
    broadcast_ready = $false
    chain_broadcast = $false
}
$json = $manifest | ConvertTo-Json -Depth 6
$parent = Split-Path $outputFullPath -Parent
if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    throw "Output parent does not exist: $parent"
}
[IO.File]::WriteAllText($outputFullPath, $json + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
Write-Output $outputFullPath
