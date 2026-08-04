$ErrorActionPreference = 'Stop'

Write-Host 'Building and running the three-party local instance test...' -ForegroundColor Cyan
cargo run --example three_party_demo
if ($LASTEXITCODE -ne 0) {
    throw "three_party_demo failed with exit code $LASTEXITCODE"
}
