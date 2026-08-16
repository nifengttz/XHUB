$ErrorActionPreference = "Stop"

$validator = Join-Path $PSScriptRoot "validate-canary-plan.ps1"
$example = Join-Path $PSScriptRoot "canary-plan.example.json"

function Assert-Rejected([string]$Name, [scriptblock]$Mutation) {
    $plan = Get-Content -LiteralPath $example -Raw | ConvertFrom-Json
    $plan.funding_coin_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    $plan.funding_puzzle_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    & $Mutation $plan
    $path = Join-Path $env:TEMP ("xhub-canary-" + [guid]::NewGuid().ToString() + ".json")
    try {
        $plan | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -PlanPath $path *> $null
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousPreference
        if ($exitCode -eq 0) { throw "Invalid canary plan was accepted: $Name" }
    } finally {
        Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    }
}

$valid = Get-Content -LiteralPath $example -Raw | ConvertFrom-Json
$valid.funding_coin_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
$valid.funding_puzzle_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
$validPath = Join-Path $env:TEMP ("xhub-canary-valid-" + [guid]::NewGuid().ToString() + ".json")
try {
    $valid | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $validPath -Encoding utf8
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -PlanPath $validPath
    if ($LASTEXITCODE -ne 0) { throw "Valid canary plan was rejected" }
} finally {
    Remove-Item -LiteralPath $validPath -Force -ErrorAction SilentlyContinue
}

Assert-Rejected "placeholder coin" { param($plan) $plan.funding_coin_id = "REPLACE_WITH_MAINNET_64_HEX_FUNDING_COIN_ID" }
Assert-Rejected "two mojo limit" { param($plan) $plan.max_total_mojo = 2 }
Assert-Rejected "broadcast enabled" { param($plan) $plan.broadcast_enabled = $true }
Assert-Rejected "missing manual approval" { param($plan) $plan.manual_approval_required = $false }
Assert-Rejected "missing review" { param($plan) $plan.required_checks = @("mainnet_rpc_preflight_verified") }

Write-Output "CANARY_PLAN_TESTS_OK"
