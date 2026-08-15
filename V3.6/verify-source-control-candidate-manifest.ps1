param(
    [string]$RepositoryRoot = (Split-Path $PSScriptRoot -Parent),
    [string]$ManifestPath = (Join-Path $PSScriptRoot "source-control-candidate-manifest.json")
)

$ErrorActionPreference = "Stop"
$generator = Join-Path $PSScriptRoot "generate-source-control-candidate-manifest.ps1"
$temporary = Join-Path $env:TEMP ("xhub-v36-source-manifest-" + [guid]::NewGuid() + ".json")
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generator `
        -RepositoryRoot $RepositoryRoot -OutputPath $temporary | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Candidate manifest regeneration failed" }
    $expected = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    $actual = Get-Content -LiteralPath $temporary -Raw | ConvertFrom-Json
    if ($expected.schema -ne "xhub-v3-6-source-control-candidate-manifest-1") { throw "Manifest schema mismatch" }
    if ($expected.candidate_file_count -ne $actual.candidate_file_count) { throw "Candidate count mismatch" }
    if ($expected.candidate_total_bytes -ne $actual.candidate_total_bytes) { throw "Candidate byte count mismatch" }
    if ($expected.tree_sha256 -ne $actual.tree_sha256) { throw "Candidate tree hash mismatch" }
    if (@($expected.files).Count -ne @($actual.files).Count) { throw "Candidate entry count mismatch" }
    for ($index = 0; $index -lt @($expected.files).Count; $index++) {
        $left = $expected.files[$index]
        $right = $actual.files[$index]
        if ($left.path -ne $right.path -or $left.size -ne $right.size -or $left.sha256 -ne $right.sha256) {
            throw "Candidate entry mismatch at index $index"
        }
    }
    if ($expected.commit_created -ne $false -or $expected.automatic_staging_enabled -ne $false) {
        throw "Manifest weakened Git mutation boundaries"
    }
    [ordered]@{
        schema = "xhub-v3-6-source-control-candidate-manifest-verification-1"
        protocol_version = "0x0360"
        status = "VERIFIED"
        candidate_file_count = $actual.candidate_file_count
        candidate_total_bytes = $actual.candidate_total_bytes
        tree_sha256 = $actual.tree_sha256
        all_entries_match = $true
        commit_created = $false
        automatic_staging_enabled = $false
        manual_review_required = $true
        production_ready = $false
        spend_bundle_created = $false
        broadcast_enabled = $false
        broadcast_ready = $false
        chain_broadcast = $false
    } | ConvertTo-Json -Depth 4
} finally {
    Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
}
