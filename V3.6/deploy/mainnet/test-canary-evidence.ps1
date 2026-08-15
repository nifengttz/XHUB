$ErrorActionPreference = "Stop"

$validator = Join-Path $PSScriptRoot "validate-canary-evidence.ps1"
$plan = Join-Path $PSScriptRoot "canary-plan.example.json"
$manifest = Join-Path $PSScriptRoot "..\..\puzzles-v3_6\module-hashes.json"
$planObject = Get-Content -LiteralPath $plan -Raw | ConvertFrom-Json
$planObject.funding_coin_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
$planObject.funding_puzzle_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
$tempRoot = Join-Path $env:TEMP ("xhub-evidence-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $planLocal = Join-Path $tempRoot "plan.json"
    $evidenceLocal = Join-Path $tempRoot "evidence.json"
    $planObject | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $planLocal -Encoding utf8
    $planHash = (Get-FileHash -LiteralPath $planLocal -Algorithm SHA256).Hash.ToLowerInvariant()
    $moduleObject = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
    $hashes = [ordered]@{}
    foreach ($property in $moduleObject.modules.PSObject.Properties) { $hashes[$property.Name] = $property.Value.module_hash }
    $evidenceObject = [ordered]@{
        schema = "xhub-v3-6-mainnet-canary-evidence-1"
        protocol_version = "0x0360"
        plan_sha256 = $planHash
        network_id = "1111111111111111111111111111111111111111111111111111111111111111"
        funding_coin_id = $planObject.funding_coin_id
        funding_puzzle_hash = $planObject.funding_puzzle_hash
        preflight = @{ status = "VERIFIED"; funding_coin_confirmations = 32; snapshot_hash = "2222222222222222222222222222222222222222222222222222222222222222" }
        module_hashes = $hashes
        recovery_simulation = @{ status = "VERIFIED"; package_content_hash = "3333333333333333333333333333333333333333333333333333333333333333" }
        approval_records = @(
            @{ reviewer_id = "reviewer-a"; failure_domain = "domain-a"; status = "APPROVED" },
            @{ reviewer_id = "reviewer-b"; failure_domain = "domain-b"; status = "APPROVED" }
        )
        broadcast_enabled = $false
        external_broadcast_authorized = $false
    }
    $evidenceObject | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $evidenceLocal -Encoding utf8
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -EvidencePath $evidenceLocal -PlanPath $planLocal -ModuleManifestPath $manifest
    if ($LASTEXITCODE -ne 0) { throw "Valid evidence package was rejected" }

    $bad = $evidenceObject | ConvertTo-Json -Depth 10 | ConvertFrom-Json
    $bad.broadcast_enabled = $true
    $bad | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $evidenceLocal -Encoding utf8
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -EvidencePath $evidenceLocal -PlanPath $planLocal -ModuleManifestPath $manifest *> $null
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference
    if ($exitCode -eq 0) { throw "Broadcast-enabled evidence was accepted" }
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "CANARY_EVIDENCE_TESTS_OK"
