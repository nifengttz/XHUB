param(
    [Parameter(Mandatory = $true)][string]$RuntimePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$TlsProfilePath,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-runtime.ps1"
$validation = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator `
    -RuntimePath $RuntimePath -IdentityManifestPath $IdentityManifestPath -TlsProfilePath $TlsProfilePath | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $validation.status -ne "WATCHTOWER_RUNTIME_PLAN_VALIDATED_ONLY") {
    throw "Watchtower runtime validation failed"
}
if (Test-Path -LiteralPath $OutputDirectory) { throw "Output directory already exists: $OutputDirectory" }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$runtimeResolved = (Resolve-Path -LiteralPath $RuntimePath).Path
$runtime = Get-Content -LiteralPath $runtimeResolved -Raw | ConvertFrom-Json
$generated = @()
foreach ($node in @($runtime.nodes)) {
    $id = ([string]$node.attester_id).ToLowerInvariant()
    $databaseDirectory = ([string]$node.database_path).Substring(0, ([string]$node.database_path).LastIndexOf('/'))
    $unit = @"
[Unit]
Description=XHUB Watchtower V3.6 ($id)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$($node.service_account)
Group=$($node.service_account)
WorkingDirectory=$($node.working_directory)
ExecStart=$($node.executable_path)
Environment=XHUB_WATCHTOWER_LISTEN=$($node.listen)
Environment=XHUB_WATCHTOWER_DB=$($node.database_path)
Environment=XHUB_WATCHTOWER_API_TOKEN_FILE=$($node.api_token_file)
Environment=XHUB_WATCHTOWER_CONFIRMERS_FILE=$($node.confirmers_file)
Environment=XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE=$($node.custody_attesters_file)
Restart=on-failure
RestartSec=$($node.restart_delay_seconds)s
TimeoutStopSec=$($node.stop_timeout_seconds)s
UMask=0077
LimitNOFILE=$($node.limit_nofile)
MemoryMax=$($node.memory_max_mb)M
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
LockPersonality=true
RestrictSUIDSGID=true
CapabilityBoundingSet=
AmbientCapabilities=
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
ReadWritePaths=$databaseDirectory $($node.backup_directory)
ReadOnlyPaths=$($node.confirmers_file) $($node.custody_attesters_file) $($node.api_token_file)

[Install]
WantedBy=multi-user.target
"@
    $fileName = "xhub-watchtower-v3-6-$id.service"
    $path = Join-Path $OutputDirectory $fileName
    [System.IO.File]::WriteAllText($path, ($unit.TrimStart() + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
    $generated += [ordered]@{file=$fileName;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
}
$manifest = [ordered]@{
    schema="xhub-v3-6-systemd-generation-1";protocol_version="0x0360"
    runtime_sha256=(Get-FileHash -LiteralPath $runtimeResolved -Algorithm SHA256).Hash.ToLowerInvariant()
    executable_sha256=([string]$runtime.executable_sha256).ToLowerInvariant();files=$generated
    secrets_embedded=$false;installed=$false;services_started=$false
    production_approved=$false;production_broadcast=$false;status="SYSTEMD_UNITS_GENERATED_NOT_INSTALLED"
}
[System.IO.File]::WriteAllText((Join-Path $OutputDirectory "systemd-manifest.json"), (($manifest|ConvertTo-Json -Depth 6)+[Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
Write-Output "Generated $OutputDirectory"
