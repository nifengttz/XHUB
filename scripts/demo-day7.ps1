$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    Write-Host '=== Compile CLVM ==='
    & (Join-Path $PSScriptRoot 'compile-puzzles.ps1')
    if ($LASTEXITCODE -ne 0) { throw "CLVM compilation failed: $LASTEXITCODE" }

    Write-Host "`n=== Verify protocol vectors ==="
    & (Join-Path $PSScriptRoot 'verify-day1.ps1')
    if ($LASTEXITCODE -ne 0) { throw "Protocol verification failed: $LASTEXITCODE" }

    Write-Host "`n=== Run clean simulator demo ==="
    cargo run --quiet --example day7_demo
    if ($LASTEXITCODE -ne 0) { throw "Day 7 demo failed: $LASTEXITCODE" }

    Write-Host "`n=== Run full attack and regression suite ==="
    cargo test --all-targets
    if ($LASTEXITCODE -ne 0) { throw "Test suite failed: $LASTEXITCODE" }

    Write-Host "`n=== Run strict lint ==="
    cargo clippy --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "Clippy failed: $LASTEXITCODE" }

    Write-Host "`nWALL-HUB MVP FINAL RESULT: PASS"
}
finally {
    Pop-Location
}
