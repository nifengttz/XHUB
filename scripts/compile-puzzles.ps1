$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$symbolDirectory = Join-Path $repoRoot 'target\clvm'

if (-not (Get-Command run -ErrorAction SilentlyContinue)) {
    throw 'Missing `run`. Install with: cargo install clvm_tools_rs --version 0.4.0 --locked'
}

[IO.Directory]::CreateDirectory($symbolDirectory) | Out-Null
foreach ($name in @('wall_hub_channel_v1', 'wall_hub_channel_v2')) {
    $source = Join-Path $repoRoot "puzzles\$name.clsp"
    $output = Join-Path $repoRoot "puzzles\$name.clsp.hex"
    $symbolOutput = Join-Path $symbolDirectory "$name.sym"
    $compiledSExpression = (& run $source --strict --optimize --symbol-output-file $symbolOutput 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Chialisp compilation failed: $compiledSExpression" }
    $compiled = (& opc $compiledSExpression 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $compiled -notmatch '^[0-9a-fA-F]+$' -or ($compiled.Length % 2) -ne 0) {
        throw "Compiler did not return one hexadecimal CLVM program: $compiled"
    }
    [IO.File]::WriteAllText($output, $compiled.ToLowerInvariant() + [Environment]::NewLine)
    Write-Host "Compiled $source"
    Write-Host "Wrote    $output"
}
