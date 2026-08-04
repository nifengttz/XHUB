param(
    [int]$Seconds = 300
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not (Get-Command cargo-fuzz -ErrorAction SilentlyContinue)) {
    throw 'Missing cargo-fuzz. Install with: cargo install cargo-fuzz --locked'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    foreach ($target in @('protocol_bytes', 'clvm_solution')) {
        cargo fuzz run $target -- -max_total_time=$Seconds
        if ($LASTEXITCODE -ne 0) {
            throw "Fuzz target failed: $target"
        }
    }
}
finally {
    Pop-Location
}
