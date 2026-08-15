param(
    [Parameter(Mandatory = $true)][string]$SimulationPath,
    [Parameter(Mandatory = $true)][string]$PlanPath,
    [Parameter(Mandatory = $true)][string]$BindingPath
)

$ErrorActionPreference = "Stop"
$simulationText = Get-Content -LiteralPath (Resolve-Path $SimulationPath) -Raw
if ($simulationText -match '(?i)private.?key|mnemonic|spend.?bundle.*hex|push_tx') { throw "Recovery simulation contains prohibited material" }
$report = $simulationText | ConvertFrom-Json
$plan = Get-Content -LiteralPath (Resolve-Path $PlanPath) -Raw | ConvertFrom-Json
$binding = Get-Content -LiteralPath (Resolve-Path $BindingPath) -Raw | ConvertFrom-Json
if ($report.schema -ne "xhub-v3-6-mainnet-closing-simulation-1" -or $report.mainnet_approved -ne $false) { throw "Unsupported recovery simulation report" }
if ($report.chain_broadcast -ne $false -or $report.spend_bundle_created -ne $false) { throw "Recovery simulation created or broadcast a SpendBundle" }
if ($report.simulation.protocol_version -ne "0x0360" -or $report.simulation.funding_coin_id -ne $plan.funding_coin_id) { throw "Recovery simulation differs from plan" }
if ($binding.status -ne "FUNDING_BINDING_VERIFIED" -or $binding.funding_coin_id -ne $plan.funding_coin_id) { throw "Funding binding is not verified" }
if ($report.simulation.recovery_package_verified -ne $true -or $report.simulation.all_clvm_conditions_verified -ne $true) { throw "RecoveryPackage or CLVM simulation is not verified" }
if ($report.simulation.broadcast_ready -ne $false -or $report.simulation.chain_broadcast -ne $false) { throw "Simulation broadcast flags must remain false" }
if ([uint64]$report.simulation.funding_amount_mojo -ne [uint64]$plan.max_total_mojo) { throw "Simulation amount differs from canary plan" }
if ([string]$report.recovery_package_content_hash -notmatch '^[0-9a-fA-F]{64}$') { throw "RecoveryPackage content hash is invalid" }
[pscustomobject]@{
    schema = "xhub-v3-6-mainnet-recovery-simulation-evidence-1"
    protocol_version = "0x0360"
    simulation_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $SimulationPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    plan_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $PlanPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    binding_sha256 = (Get-FileHash -LiteralPath (Resolve-Path $BindingPath) -Algorithm SHA256).Hash.ToLowerInvariant()
    funding_coin_id = $plan.funding_coin_id
    recovery_package_content_hash = $report.recovery_package_content_hash
    broadcast_enabled = $false
    status = "RECOVERY_SIMULATION_VERIFIED"
} | ConvertTo-Json -Depth 5
