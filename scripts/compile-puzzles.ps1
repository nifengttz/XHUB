$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'puzzles\wall_hub_channel_v1.clsp'
$output = Join-Path $repoRoot 'puzzles\wall_hub_channel_v1.clsp.hex'
$symbolDirectory = Join-Path $repoRoot 'target\clvm'
$symbolOutput = Join-Path $symbolDirectory 'wall_hub_channel_v1.sym'

if (-not (Get-Command run -ErrorAction SilentlyContinue)) {
    throw 'Missing `run`. Install with: cargo install clvm_tools_rs --version 0.4.0 --locked'
}

[IO.Directory]::CreateDirectory($symbolDirectory) | Out-Null
$compiledSExpression = (& run $source --strict --optimize --symbol-output-file $symbolOutput 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Chialisp compilation failed: $compiledSExpression"
}

$compiled = (& opc $compiledSExpression 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "CLVM assembly failed: $compiled"
}
if ($compiled -notmatch '^[0-9a-fA-F]+$' -or ($compiled.Length % 2) -ne 0) {
    throw "Compiler did not return one hexadecimal CLVM program: $compiled"
}

[IO.File]::WriteAllText($output, $compiled.ToLowerInvariant() + [Environment]::NewLine)
Write-Host "Compiled $source"
Write-Host "Wrote    $output"
