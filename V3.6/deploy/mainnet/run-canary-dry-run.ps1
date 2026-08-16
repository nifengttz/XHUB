param(
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$RpcOutputPath,
    [Parameter(Mandatory = $true)][string]$PlanPath,
    [Parameter(Mandatory = $true)][string]$BindingPath,
    [Parameter(Mandatory = $true)][string]$SimulationPath,
    [Parameter(Mandatory = $true)][string]$EvidencePath,
    [Parameter(Mandatory = $true)][string]$ApprovalPath,
    [Parameter(Mandatory = $true)][string]$OutputPath
)

$ErrorActionPreference = "Stop"
function Invoke-JsonCheck([string]$Script, [string[]]$Arguments) {
    $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot $Script) @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Dry-run check failed: $Script" }
    $output | ConvertFrom-Json
}

$parameters = Invoke-JsonCheck "validate-mainnet-parameters.ps1" @("-ParametersPath", $ParametersPath)
$rpc = Invoke-JsonCheck "validate-rpc-preflight-output.ps1" @("-RpcOutputPath", $RpcOutputPath, "-ConfigPath", $ConfigPath)
$plan = Invoke-JsonCheck "validate-canary-plan.ps1" @("-PlanPath", $PlanPath)
$binding = Invoke-JsonCheck "validate-funding-binding.ps1" @("-RpcOutputPath", $RpcOutputPath, "-PlanPath", $PlanPath, "-ConfigPath", $ConfigPath, "-ParametersPath", $ParametersPath)
$simulation = Invoke-JsonCheck "validate-recovery-simulation.ps1" @("-SimulationPath", $SimulationPath, "-PlanPath", $PlanPath, "-BindingPath", $BindingPath)
$evidence = Invoke-JsonCheck "validate-canary-evidence.ps1" @("-EvidencePath", $EvidencePath, "-PlanPath", $PlanPath)
$approvals = Invoke-JsonCheck "validate-approval-records.ps1" @("-ApprovalPath", $ApprovalPath, "-EvidencePath", $EvidencePath)

if ($binding.status -ne "FUNDING_BINDING_VERIFIED" -or $simulation.status -ne "RECOVERY_SIMULATION_VERIFIED") { throw "Dry-run binding or simulation did not verify" }
$report = [ordered]@{
    schema = "xhub-v3-6-mainnet-canary-dry-run-1"
    protocol_version = "0x0360"
    checks = [ordered]@{
        parameters = $parameters.status
        rpc = $rpc.status
        plan = $plan.status
        funding_binding = $binding.status
        recovery_simulation = $simulation.status
        evidence = $evidence.status
        approvals = $approvals.status
    }
    input_hashes = [ordered]@{
        config = (Get-FileHash (Resolve-Path $ConfigPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        parameters = (Get-FileHash (Resolve-Path $ParametersPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        rpc_output = (Get-FileHash (Resolve-Path $RpcOutputPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        plan = (Get-FileHash (Resolve-Path $PlanPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        binding = (Get-FileHash (Resolve-Path $BindingPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        simulation = (Get-FileHash (Resolve-Path $SimulationPath) -Algorithm SHA256).Hash.ToLowerInvariant()
        evidence = (Get-FileHash (Resolve-Path $EvidencePath) -Algorithm SHA256).Hash.ToLowerInvariant()
        approvals = (Get-FileHash (Resolve-Path $ApprovalPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    spend_bundle_created = $false
    broadcast_enabled = $false
    chain_broadcast = $false
    status = "DRY_RUN_COMPLETE_MANUAL_REVIEW_REQUIRED"
}
[System.IO.File]::WriteAllText($OutputPath, (($report | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
Get-Content -LiteralPath $OutputPath -Raw
