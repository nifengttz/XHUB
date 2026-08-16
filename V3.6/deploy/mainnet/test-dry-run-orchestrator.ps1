$ErrorActionPreference = "Stop"
$scriptText = Get-Content -LiteralPath (Join-Path $PSScriptRoot "run-canary-dry-run.ps1") -Raw
foreach ($required in @("validate-mainnet-parameters.ps1", "validate-rpc-preflight-output.ps1", "validate-canary-plan.ps1", "validate-funding-binding.ps1", "validate-recovery-simulation.ps1", "validate-canary-evidence.ps1", "validate-approval-records.ps1", "DRY_RUN_COMPLETE_MANUAL_REVIEW_REQUIRED")) {
    if (-not $scriptText.Contains($required)) { throw "Dry-run orchestrator is missing: $required" }
}
if ($scriptText -match '(?i)push_tx|broadcast_enabled\s*=\s*\$true|chain_broadcast\s*=\s*\$true|spend_bundle_created\s*=\s*\$true') {
    throw "Dry-run orchestrator contains a broadcast path"
}

$tempRoot = Join-Path $env:TEMP ("xhub-dry-run-" + [guid]::NewGuid())
New-Item -ItemType Directory $tempRoot | Out-Null
try {
    $coin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    $puzzle = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    $network = "ccd5bb71183532bff220ba46c268991a0000bfec0d2214bdd7847734b38a4e87"
    $paths = @{}
    foreach ($name in @("config", "rpc", "plan", "binding", "simulation", "evidence", "approvals", "report")) { $paths[$name] = Join-Path $tempRoot "$name.json" }
    [ordered]@{expected_network_id=$network;funding_coin_id=$coin} | ConvertTo-Json | Set-Content $paths.config -Encoding utf8
    [ordered]@{schema="xhub-v3-6-rpc-preflight-1";protocol_version="0x0360";network_id=$network;synced=$true;peak_height=1000;funding_coin=[ordered]@{status="CONFIRMED";birth_height=969;confirmations=32;puzzle_hash=$puzzle;amount=1};required_funding_confirmations=32;ready=$true} | ConvertTo-Json -Depth 6 | Set-Content $paths.rpc -Encoding utf8
    [ordered]@{schema="xhub-v3-6-mainnet-canary-plan-1";protocol_version="0x0360";environment="mainnet-canary";funding_coin_id=$coin;funding_puzzle_hash=$puzzle;max_total_mojo=1;broadcast_enabled=$false;manual_approval_required=$true;required_checks=@("mainnet_rpc_preflight_verified","funding_coin_record_verified","puzzle_hash_and_module_hashes_verified","recovery_package_delivery_simulated","two_person_review_recorded");prohibited_materials=@("private_key","mnemonic","spend_bundle_canonical_hex","push_tx")} | ConvertTo-Json -Depth 8 | Set-Content $paths.plan -Encoding utf8
    $parameters = Join-Path $PSScriptRoot "mainnet-parameters.candidate.json"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-funding-binding.ps1") -RpcOutputPath $paths.rpc -PlanPath $paths.plan -ConfigPath $paths.config -ParametersPath $parameters | Set-Content $paths.binding -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "Dry-run fixture binding failed" }
    [ordered]@{schema="xhub-v3-6-mainnet-closing-simulation-1";release_status="UNAUDITED_MAINNET_EXPERIMENT";mainnet_approved=$false;network="mainnet";chain_broadcast=$false;spend_bundle_created=$false;recovery_package_content_hash=("33"*32);simulation=[ordered]@{protocol_version="0x0360";funding_coin_id=$coin;funding_amount_mojo=1;recovery_package_verified=$true;all_clvm_conditions_verified=$true;broadcast_ready=$false;chain_broadcast=$false}} | ConvertTo-Json -Depth 8 | Set-Content $paths.simulation -Encoding utf8
    $moduleManifest = Get-Content (Join-Path $PSScriptRoot "..\..\puzzles-v3_6\module-hashes.json") -Raw | ConvertFrom-Json
    $moduleHashes = [ordered]@{}; foreach($property in $moduleManifest.modules.PSObject.Properties){$moduleHashes[$property.Name]=$property.Value.module_hash}
    $planHash = (Get-FileHash $paths.plan -Algorithm SHA256).Hash.ToLowerInvariant()
    [ordered]@{schema="xhub-v3-6-mainnet-canary-evidence-1";protocol_version="0x0360";plan_sha256=$planHash;network_id=$network;funding_coin_id=$coin;funding_puzzle_hash=$puzzle;preflight=[ordered]@{status="VERIFIED";funding_coin_confirmations=32;snapshot_hash=("22"*32)};module_hashes=$moduleHashes;recovery_simulation=[ordered]@{status="VERIFIED";package_content_hash=("33"*32)};approval_records=@([ordered]@{reviewer_id="reviewer-a";failure_domain="domain-a";status="APPROVED"},[ordered]@{reviewer_id="reviewer-b";failure_domain="domain-b";status="APPROVED"});broadcast_enabled=$false;external_broadcast_authorized=$false} | ConvertTo-Json -Depth 10 | Set-Content $paths.evidence -Encoding utf8
    $evidenceHash=(Get-FileHash $paths.evidence -Algorithm SHA256).Hash.ToLowerInvariant()
    [ordered]@{schema="xhub-v3-6-mainnet-canary-approvals-1";protocol_version="0x0360";evidence_sha256=$evidenceHash;records=@([ordered]@{reviewer_id="reviewer-a";failure_domain="domain-a";decision="APPROVED";reviewed_evidence_sha256=$evidenceHash},[ordered]@{reviewer_id="reviewer-b";failure_domain="domain-b";decision="APPROVED";reviewed_evidence_sha256=$evidenceHash});broadcast_authorized=$false} | ConvertTo-Json -Depth 8 | Set-Content $paths.approvals -Encoding utf8
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "run-canary-dry-run.ps1") -ConfigPath $paths.config -ParametersPath $parameters -RpcOutputPath $paths.rpc -PlanPath $paths.plan -BindingPath $paths.binding -SimulationPath $paths.simulation -EvidencePath $paths.evidence -ApprovalPath $paths.approvals -OutputPath $paths.report | Out-Null
    $report=Get-Content $paths.report -Raw|ConvertFrom-Json
    if($LASTEXITCODE -ne 0 -or $report.status -ne "DRY_RUN_COMPLETE_MANUAL_REVIEW_REQUIRED" -or $report.broadcast_enabled -ne $false){throw "Functional dry-run failed"}
} finally { Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue }
Write-Output "DRY_RUN_ORCHESTRATOR_STATIC_TESTS_OK"
