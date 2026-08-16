$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$puzzleRoot = $PSScriptRoot
$symbolDirectory = Join-Path $puzzleRoot 'build\symbols'
$hashOutput = Join-Path $puzzleRoot 'module-hashes.json'
$names = @(
    'xhub_funding_v3_6',
    'xhub_initial_closing_v3_6',
    'xhub_subsequent_closing_v3_6',
    'xhub_merchant_payment_v3_6'
)

foreach ($command in @('run', 'opc')) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Missing '$command'. Install clvm_tools_rs 0.4.0."
    }
}

[IO.Directory]::CreateDirectory($symbolDirectory) | Out-Null
$modules = [ordered]@{}

Push-Location $puzzleRoot
try {
    foreach ($name in $names) {
        $source = Join-Path $puzzleRoot "$name.clsp"
        $hexOutput = Join-Path $puzzleRoot "$name.clsp.hex"
        $symbolOutput = Join-Path $symbolDirectory "$name.sym"
        $compiledExpression = (& run $source -i $puzzleRoot --strict --optimize --symbol-output-file $symbolOutput 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $compiledExpression -match 'Unbound use|FAIL:|Error:') {
            throw "Chialisp compilation failed for $name`: $compiledExpression"
        }

        $compiledHex = (& opc $compiledExpression 2>&1 | Out-String).Trim().ToLowerInvariant()
        if ($LASTEXITCODE -ne 0 -or $compiledHex -notmatch '^[0-9a-f]+$' -or ($compiledHex.Length % 2) -ne 0) {
            throw "Compiler did not return one hexadecimal program for $name`: $compiledHex"
        }
        $moduleHash = (& opc -H $compiledExpression 2>&1 | Out-String).Trim().ToLowerInvariant()
        if ($LASTEXITCODE -ne 0 -or $moduleHash -notmatch '^[0-9a-f]{64}$') {
            throw "Compiler did not return a bytes32 module hash for $name`: $moduleHash"
        }

        [IO.File]::WriteAllText($hexOutput, $compiledHex + [Environment]::NewLine)
        $modules[$name] = [ordered]@{
            source = "$name.clsp"
            hex = "$name.clsp.hex"
            byte_length = $compiledHex.Length / 2
            module_hash = $moduleHash
        }
        Write-Host "Compiled $name -> $moduleHash"
    }
}
finally {
    Pop-Location
}

$artifact = [ordered]@{
    schema = 'xhub-v3-6-clvm-modules-1'
    protocol_version = '0x0360'
    compiler = [ordered]@{
        run = ((& run --version 2>&1 | Out-String).Trim())
        opc = ((& opc --version 2>&1 | Out-String).Trim())
        flags = @('--strict', '--optimize')
    }
    modules = $modules
}
[IO.File]::WriteAllText(
    $hashOutput,
    ($artifact | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
    [Text.UTF8Encoding]::new($false)
)
Write-Host "Wrote $hashOutput"
