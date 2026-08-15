$ErrorActionPreference = "Stop"
$tempRoot = Join-Path $env:TEMP ("xhub-chain-evidence-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $coin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    $puzzle = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    $network = "ccd5bb71183532bff220ba46c268991a0000bfec0d2214bdd7847734b38a4e87"
    $configPath = Join-Path $tempRoot "config.json"
    $planPath = Join-Path $tempRoot "plan.json"
    $rpcPath = Join-Path $tempRoot "rpc.json"
    $bindingPath = Join-Path $tempRoot "binding.json"
    $simulationPath = Join-Path $tempRoot "simulation.json"
    $parametersPath = Join-Path $PSScriptRoot "mainnet-parameters.candidate.json"
    [ordered]@{ expected_network_id = $network; funding_coin_id = $coin } | ConvertTo-Json | Set-Content $configPath -Encoding utf8
    [ordered]@{ schema="xhub-v3-6-mainnet-canary-plan-1"; protocol_version="0x0360"; funding_coin_id=$coin; funding_puzzle_hash=$puzzle; max_total_mojo=1; broadcast_enabled=$false } | ConvertTo-Json | Set-Content $planPath -Encoding utf8
    [ordered]@{
        schema="xhub-v3-6-rpc-preflight-1"; protocol_version="0x0360"; network_id=$network; synced=$true; peak_height=1000
        funding_coin=[ordered]@{status="CONFIRMED";birth_height=969;confirmations=32;puzzle_hash=$puzzle;amount=1}
        required_funding_confirmations=32;ready=$true
    } | ConvertTo-Json -Depth 6 | Set-Content $rpcPath -Encoding utf8

    $rpcEvidence = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-rpc-preflight-output.ps1") -RpcOutputPath $rpcPath -ConfigPath $configPath
    if ($LASTEXITCODE -ne 0 -or ($rpcEvidence | ConvertFrom-Json).status -ne "RPC_PREFLIGHT_EVIDENCE_VERIFIED") { throw "RPC evidence validation failed" }
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-funding-binding.ps1") -RpcOutputPath $rpcPath -PlanPath $planPath -ConfigPath $configPath -ParametersPath $parametersPath | Set-Content $bindingPath -Encoding utf8
    if ($LASTEXITCODE -ne 0 -or (Get-Content $bindingPath -Raw | ConvertFrom-Json).status -ne "FUNDING_BINDING_VERIFIED") { throw "Funding binding validation failed" }
    [ordered]@{
        schema="xhub-v3-6-mainnet-closing-simulation-1";release_status="UNAUDITED_MAINNET_EXPERIMENT";mainnet_approved=$false;network="mainnet"
        chain_broadcast=$false;spend_bundle_created=$false;recovery_package_content_hash=("33" * 32)
        simulation=[ordered]@{protocol_version="0x0360";funding_coin_id=$coin;funding_amount_mojo=1;recovery_package_verified=$true;all_clvm_conditions_verified=$true;broadcast_ready=$false;chain_broadcast=$false}
    } | ConvertTo-Json -Depth 8 | Set-Content $simulationPath -Encoding utf8
    $recoveryEvidence = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-recovery-simulation.ps1") -SimulationPath $simulationPath -PlanPath $planPath -BindingPath $bindingPath
    if ($LASTEXITCODE -ne 0 -or ($recoveryEvidence | ConvertFrom-Json).status -ne "RECOVERY_SIMULATION_VERIFIED") { throw "Recovery simulation validation failed" }

    $badRpc = Get-Content $rpcPath -Raw | ConvertFrom-Json
    $badRpc.ready = $false
    $badRpc | ConvertTo-Json -Depth 6 | Set-Content $rpcPath -Encoding utf8
    $previous = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-rpc-preflight-output.ps1") -RpcOutputPath $rpcPath -ConfigPath $configPath *> $null
    $exitCode = $LASTEXITCODE; $ErrorActionPreference = $previous
    if ($exitCode -eq 0) { throw "Not-ready RPC evidence was accepted" }
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "CHAIN_EVIDENCE_TESTS_OK"
