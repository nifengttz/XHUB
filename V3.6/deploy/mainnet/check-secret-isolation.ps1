param(
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [string]$ProbeProfilePath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-MainnetConfig $ConfigPath
$config = $deployment.Config
$secretEntries = @(
    [ordered]@{ name="hub_api_token_file"; base=$deployment.Directory; value=[string]$config.hub_api_token_file },
    [ordered]@{ name="watchtower_api_token_file"; base=$deployment.Directory; value=[string]$config.watchtower_api_token_file },
    [ordered]@{ name="hub_bls_secret_file"; base=$deployment.Directory; value=[string]$config.hub_bls_secret_file }
)
if ($config.rpc_mode -eq "self_hosted_mtls") {
    $secretEntries += [ordered]@{ name="chia_rpc_key_file"; base=$deployment.Directory; value=[string]$config.chia_rpc_key_file }
}
if (-not [string]::IsNullOrWhiteSpace($ProbeProfilePath)) {
    $probeResolved = (Resolve-Path -LiteralPath $ProbeProfilePath).Path
    $probeDirectory = Split-Path $probeResolved -Parent
    $probe = Get-Content -LiteralPath $probeResolved -Raw | ConvertFrom-Json
    if ($probe.schema -ne "xhub-v3-6-watchtower-endpoint-probe-1" -or @($probe.nodes).Count -ne 3) {
        throw "Unsupported Watchtower endpoint probe profile"
    }
    foreach ($node in @($probe.nodes)) {
        foreach ($field in @("api_token_file", "client_certificate_pfx_file", "client_certificate_password_file")) {
            $secretEntries += [ordered]@{
                name = "probe[$([string]$node.attester_id)].$field"
                base = $probeDirectory
                value = [string]$node.$field
            }
        }
    }
}
$paths = @{}
$results = @()
foreach ($entry in $secretEntries) {
    $name = [string]$entry.name
    $path = Resolve-ConfigPath ([string]$entry.base) ([string]$entry.value)
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Secret file is missing: $name" }
    $resolved = (Resolve-Path -LiteralPath $path).Path
    if ($paths.ContainsKey($resolved.ToLowerInvariant())) { throw "Secret files must be distinct: $name" }
    $paths[$resolved.ToLowerInvariant()] = $name
    $item = Get-Item -LiteralPath $resolved
    $minimumSize = if ($name -match '\.client_certificate_password_file$') { 16 } elseif ($name -match '\.client_certificate_pfx_file$') { 256 } else { 32 }
    $maximumSize = if ($name -match '\.client_certificate_pfx_file$') { 1048576 } else { 16384 }
    if ($item.Length -lt $minimumSize -or $item.Length -gt $maximumSize) { throw "Secret file size is outside the allowed boundary: $name" }
    $acl = Get-Acl -LiteralPath $resolved
    $broad = @($acl.Access | Where-Object {
        $_.AccessControlType -eq "Allow" -and
        $_.IdentityReference.Value -match '(?i)(Everyone|BUILTIN\\Users|Authenticated Users)' -and
        ($_.FileSystemRights.ToString() -match '(?i)(Read|Write|Modify|FullControl)')
    })
    if ($broad.Count -gt 0) { throw "Secret file grants broad access: $name" }
    $results += [ordered]@{ name = $name; size = [int64]$item.Length; acl_checked = $true }
}
[pscustomobject]@{
    schema = "xhub-v3-6-mainnet-secret-isolation-1"
    secret_file_count = $results.Count
    secrets = $results
    secret_values_disclosed = $false
    status = "SECRETS_ISOLATED"
} | ConvertTo-Json -Depth 6
