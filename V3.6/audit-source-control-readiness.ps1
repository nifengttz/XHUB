param(
    [string]$RepositoryRoot = (Split-Path $PSScriptRoot -Parent)
)

$ErrorActionPreference = "Stop"
$v36Root = (Resolve-Path -LiteralPath $PSScriptRoot).Path
$repoRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
if (-not $v36Root.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "V3.6 root must be inside the repository root"
}

$ignoreProbes = @(
    "V3.6/local-secrets/mainnet-experiment-hub-api-token.txt",
    "V3.6/mainnet-experiment/three-watchtower-canary/private/payment-1-mojo-hub-request.json",
    "V3.6/mainnet-experiment/three-watchtower-canary/closing-state-1/state-zero-pipeline/wt-a.sqlite3",
    "V3.6/watchtower-v3_6/target/debug/watchtower-v3-6.exe"
)
foreach ($probe in $ignoreProbes) {
    & git -C $repoRoot check-ignore --quiet -- $probe
    if ($LASTEXITCODE -ne 0) { throw "Sensitive or generated path is not ignored: $probe" }
}

$candidateOutput = & git -C $repoRoot ls-files --cached --others --exclude-standard -- V3.6
if ($LASTEXITCODE -ne 0) { throw "git ls-files failed" }
$reportRelativePath = "V3.6/source-control-readiness-evidence.json"
$manifestRelativePath = "V3.6/source-control-candidate-manifest.json"
$manifestVerificationRelativePath = "V3.6/source-control-candidate-manifest-verification-evidence.json"
$candidates = @($candidateOutput |
    Where-Object { $_ -and $_.Trim() } |
    ForEach-Object { $_.Replace('\', '/') } |
    Where-Object {
        $_ -ne $reportRelativePath -and
        $_ -ne $manifestRelativePath -and
        $_ -ne $manifestVerificationRelativePath
    })
if ($candidates.Count -eq 0) { throw "No V3.6 source-control candidates were found" }

$prohibitedPathPatterns = @(
    '(^|/)local-secrets/',
    '(^|/)private/',
    '(^|/)target/',
    '(^|/)\.target-[^/]+/',
    '\.local\.json$',
    '\.sqlite3(?:-wal|-shm)?$',
    '\.(?:pem|key|p12|pfx)$'
)
$prohibitedPaths = @()
foreach ($candidate in $candidates) {
    foreach ($pattern in $prohibitedPathPatterns) {
        if ($candidate -match $pattern) {
            $prohibitedPaths += $candidate
            break
        }
    }
}
if ($prohibitedPaths.Count -gt 0) {
    throw "Prohibited files remain in the Git candidate set: $($prohibitedPaths -join ', ')"
}

$textExtensions = @(
    '.clsp', '.css', '.html', '.json', '.md', '.ps1', '.py', '.rs', '.sh', '.toml', '.txt', '.yaml', '.yml'
)
$privateKeyMarkers = @(
    ('-----BEGIN ' + 'PRIVATE KEY-----'),
    ('-----BEGIN RSA ' + 'PRIVATE KEY-----'),
    ('-----BEGIN EC ' + 'PRIVATE KEY-----'),
    ('-----BEGIN OPENSSH ' + 'PRIVATE KEY-----')
)
$markerHits = @()
foreach ($candidate in $candidates) {
    $absolute = Join-Path $repoRoot $candidate
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) { continue }
    if ($textExtensions -notcontains [IO.Path]::GetExtension($absolute).ToLowerInvariant()) { continue }
    $content = Get-Content -LiteralPath $absolute -Raw -ErrorAction Stop
    foreach ($marker in $privateKeyMarkers) {
        if ($content.IndexOf($marker, [StringComparison]::Ordinal) -ge 0) {
            $markerHits += $candidate
            break
        }
    }
}
if ($markerHits.Count -gt 0) {
    throw "Private-key material marker found in Git candidates: $($markerHits -join ', ')"
}

$candidateBytes = 0L
foreach ($candidate in $candidates) {
    $absolute = Join-Path $repoRoot $candidate
    if (Test-Path -LiteralPath $absolute -PathType Leaf) {
        $candidateBytes += (Get-Item -LiteralPath $absolute).Length
    }
}

[ordered]@{
    schema = "xhub-v3-6-source-control-readiness-1"
    protocol_version = "0x0360"
    status = "READY_FOR_MANUAL_GIT_REVIEW"
    candidate_file_count = $candidates.Count
    candidate_count_excludes_report = $true
    candidate_total_bytes = $candidateBytes
    ignore_probe_count = $ignoreProbes.Count
    all_sensitive_probes_ignored = $true
    prohibited_candidate_count = 0
    private_key_marker_hit_count = 0
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
} | ConvertTo-Json -Depth 4
