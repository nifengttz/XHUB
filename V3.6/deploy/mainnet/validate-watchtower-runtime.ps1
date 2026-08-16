param(
    [Parameter(Mandatory = $true)][string]$RuntimePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$TlsProfilePath
)

$ErrorActionPreference = "Stop"

function Assert-ExactFields($Object, [string[]]$Expected, [string]$Context) {
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "|") -ne ($expectedSorted -join "|")) { throw "$Context fields do not match the frozen schema" }
}

function Assert-PosixPath([string]$Value, [string]$Field) {
    if ($Value -notmatch '^/[A-Za-z0-9._/-]+$' -or $Value.Contains("//") -or
        $Value.Split('/') -contains '..' -or $Value -match '^REPLACE_WITH_' -or
        $Value -match '^/(root|home)(/|$)') {
        throw "$Field must be an explicit normalized absolute POSIX path"
    }
}

$runtime = Get-Content -LiteralPath (Resolve-Path -LiteralPath $RuntimePath) -Raw | ConvertFrom-Json
$identities = Get-Content -LiteralPath (Resolve-Path -LiteralPath $IdentityManifestPath) -Raw | ConvertFrom-Json
$tls = Get-Content -LiteralPath (Resolve-Path -LiteralPath $TlsProfilePath) -Raw | ConvertFrom-Json
Assert-ExactFields $runtime @(
    "schema", "protocol_version", "platform", "executable_sha256", "nodes",
    "production_approved", "production_broadcast"
) "Watchtower runtime"
if ($runtime.schema -ne "xhub-v3-6-watchtower-runtime-1" -or $runtime.protocol_version -ne "0x0360" -or
    $runtime.platform -ne "linux-systemd-nginx") { throw "Unsupported Watchtower runtime profile" }
if ($runtime.production_approved -ne $false -or $runtime.production_broadcast -ne $false) {
    throw "Runtime profile cannot grant production approval or broadcast"
}
if ([string]$runtime.executable_sha256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw "executable_sha256 must be exactly 32-byte hexadecimal"
}
if ($identities.schema -ne "xhub-v3-6-watchtower-identities-1" -or
    $tls.schema -ne "xhub-v3-6-watchtower-tls-profile-1" -or
    $identities.production_approved -ne $false -or $tls.production_approved -ne $false) {
    throw "Identity and TLS profiles must be fail-closed V3.6 candidates"
}
$identityById = @{}
foreach ($entry in @($identities.attesters)) { $identityById[([string]$entry.attester_id).ToLowerInvariant()] = $entry }
$tlsById = @{}
foreach ($entry in @($tls.nodes)) { $tlsById[([string]$entry.attester_id).ToLowerInvariant()] = $entry }
$nodes = @($runtime.nodes)
if ($nodes.Count -ne 3 -or $identityById.Count -ne 3 -or $tlsById.Count -ne 3) {
    throw "Runtime, identity, and TLS profiles must each contain three nodes"
}
$nodeFields = @(
    "attester_id", "deployment_host", "service_account", "executable_path", "working_directory",
    "listen", "database_path", "api_token_file", "confirmers_file", "custody_attesters_file",
    "backup_directory", "memory_max_mb", "limit_nofile", "restart_delay_seconds",
    "stop_timeout_seconds", "no_new_privileges", "private_tmp", "protect_system", "protect_home"
)
$ids = @()
$hosts = @()
foreach ($node in $nodes) {
    Assert-ExactFields $node $nodeFields "Watchtower runtime node"
    $id = ([string]$node.attester_id).ToLowerInvariant()
    if ($id -notmatch '^[a-z0-9][a-z0-9_-]{0,63}$') { throw "attester_id is unsafe for a systemd unit name" }
    if (-not $identityById.ContainsKey($id) -or -not $tlsById.ContainsKey($id)) {
        throw "Runtime node has no matching identity and TLS profile: $id"
    }
    $identity = $identityById[$id]
    $tlsNode = $tlsById[$id]
    $expectedHost = ([Uri]$identity.api_base_url).DnsSafeHost
    if ([string]$node.deployment_host -ine $expectedHost -or
        ([Uri]$tlsNode.public_base_url).DnsSafeHost -ine $expectedHost) {
        throw "Runtime deployment host differs from identity/TLS profile: $id"
    }
    if ([string]$node.deployment_host -notmatch '^[A-Za-z0-9.-]+$' -or
        [string]$node.deployment_host -match '(?i)(^|\.)localhost$|^REPLACE_WITH_') {
        throw "deployment_host must be an explicit public DNS name"
    }
    if ([string]$node.service_account -notmatch '^[a-z_][a-z0-9_-]{0,31}$' -or
        [string]$node.service_account -match '^(root|admin|administrator|system)$') {
        throw "service_account must be a dedicated non-administrator account"
    }
    foreach ($field in @(
        "executable_path", "working_directory", "database_path", "api_token_file",
        "confirmers_file", "custody_attesters_file", "backup_directory"
    )) { Assert-PosixPath ([string]$node.$field) $field }
    $paths = @(
        [string]$node.database_path, [string]$node.api_token_file, [string]$node.confirmers_file,
        [string]$node.custody_attesters_file, [string]$node.backup_directory
    )
    if (($paths | Select-Object -Unique).Count -ne $paths.Count) { throw "Runtime data and configuration paths must be distinct" }
    if ([string]$node.database_path -notmatch '\.sqlite3$') { throw "database_path must end in .sqlite3" }
    if ([string]$node.listen -notmatch '^127\.0\.0\.1:([1-9][0-9]{0,4})$') { throw "Watchtower must listen on loopback IPv4" }
    $port = [int]([regex]::Match([string]$node.listen, '([0-9]+)$').Value)
    if ($port -gt 65535 -or ([Uri]$tlsNode.upstream_url).Authority -ne [string]$node.listen) {
        throw "Runtime listen address differs from TLS loopback upstream"
    }
    if ([int64]$node.memory_max_mb -lt 128 -or [int64]$node.memory_max_mb -gt 4096 -or
        [int64]$node.limit_nofile -lt 1024 -or [int64]$node.limit_nofile -gt 65536 -or
        [int64]$node.restart_delay_seconds -lt 1 -or [int64]$node.restart_delay_seconds -gt 60 -or
        [int64]$node.stop_timeout_seconds -lt 10 -or [int64]$node.stop_timeout_seconds -gt 300) {
        throw "Runtime resource or timeout limit is outside the frozen range"
    }
    if ($node.no_new_privileges -ne $true -or $node.private_tmp -ne $true -or
        $node.protect_system -ne "strict" -or $node.protect_home -ne $true) {
        throw "All systemd process hardening controls are required"
    }
    $ids += $id
    $hosts += ([string]$node.deployment_host).ToLowerInvariant()
}
if (($ids | Select-Object -Unique).Count -ne 3 -or ($hosts | Select-Object -Unique).Count -ne 3) {
    throw "Runtime nodes and deployment hosts must be distinct"
}

[pscustomobject]@{
    schema = "xhub-v3-6-watchtower-runtime-validation-1"
    protocol_version = "0x0360"
    platform = "linux-systemd-nginx"
    node_count = 3
    executable_sha256 = ([string]$runtime.executable_sha256).ToLowerInvariant()
    loopback_only = $true
    non_admin_accounts = $true
    systemd_hardening_required = $true
    production_approved = $false
    production_broadcast = $false
    status = "WATCHTOWER_RUNTIME_PLAN_VALIDATED_ONLY"
} | ConvertTo-Json -Depth 5
