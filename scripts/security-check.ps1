param(
    [switch]$RequireSecurityTools
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'cargo fmt check failed' }

    cargo test --locked --all-targets
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

    cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }

    & (Join-Path $PSScriptRoot 'generate-sbom.ps1')

    $audit = Get-Command cargo-audit -ErrorAction SilentlyContinue
    if ($audit) {
        cargo audit --locked
        if ($LASTEXITCODE -ne 0) { throw 'cargo audit failed' }
    } elseif ($RequireSecurityTools) {
        throw 'Missing cargo-audit. Install with: cargo install cargo-audit --locked'
    } else {
        Write-Warning 'cargo-audit not installed; vulnerability scan was skipped'
    }

    $deny = Get-Command cargo-deny -ErrorAction SilentlyContinue
    if ($deny) {
        cargo deny check
        if ($LASTEXITCODE -ne 0) { throw 'cargo deny failed' }
    } elseif ($RequireSecurityTools) {
        throw 'Missing cargo-deny. Install from https://github.com/EmbarkStudios/cargo-deny'
    } else {
        Write-Warning 'cargo-deny not installed; policy scan was skipped'
    }
}
finally {
    Pop-Location
}
