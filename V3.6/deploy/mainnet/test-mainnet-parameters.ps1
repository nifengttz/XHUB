$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-mainnet-parameters.ps1"
$example = Join-Path $PSScriptRoot "mainnet-parameters.candidate.json"

function Invoke-Validator($parameters) {
    $path = Join-Path $env:TEMP ("xhub-mainnet-parameters-" + [guid]::NewGuid() + ".json")
    try {
        $parameters | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ParametersPath $path *> $null
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        $exitCode
    } finally {
        Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    }
}

$valid = Get-Content -LiteralPath $example -Raw | ConvertFrom-Json
if ((Invoke-Validator $valid) -ne 0) { throw "Valid candidate parameters were rejected" }

foreach ($case in @(
    @{ name = "wrong close delay"; mutate = { param($p) $p.close_delay_blocks = 12489 } },
    @{ name = "zero challenge"; mutate = { param($p) $p.challenge_blocks = 0 } },
    @{ name = "missing merchant receipt"; mutate = { param($p) $p.merchant_delivery_confirmations_required = 0 } },
    @{ name = "testnet custody threshold"; mutate = { param($p) $p.custody_attestation_threshold = 1 } },
    @{ name = "mutable parameters"; mutate = { param($p) $p.immutable_after_funding = $false } },
    @{ name = "self-approved mainnet"; mutate = { param($p) $p.mainnet_approved = $true } }
)) {
    $candidate = Get-Content -LiteralPath $example -Raw | ConvertFrom-Json
    & $case.mutate $candidate
    if ((Invoke-Validator $candidate) -eq 0) { throw "Invalid candidate accepted: $($case.name)" }
}

Write-Output "MAINNET_PARAMETER_TESTS_OK"
