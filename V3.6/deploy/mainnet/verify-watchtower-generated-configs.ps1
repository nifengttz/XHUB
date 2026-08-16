param(
    [Parameter(Mandatory = $true)][string]$SystemdDirectory,
    [Parameter(Mandatory = $true)][string]$NginxDirectory,
    [Parameter(Mandatory = $true)][string]$RuntimePath,
    [Parameter(Mandatory = $true)][string]$TlsProfilePath
)

$ErrorActionPreference="Stop"
$systemd=(Resolve-Path -LiteralPath $SystemdDirectory).Path
$nginx=(Resolve-Path -LiteralPath $NginxDirectory).Path
$runtime=(Resolve-Path -LiteralPath $RuntimePath).Path
$tls=(Resolve-Path -LiteralPath $TlsProfilePath).Path
$systemdManifestPath=Join-Path $systemd "systemd-manifest.json"
$nginxManifestPath=Join-Path $nginx "nginx-manifest.json"
$systemdManifest=Get-Content -LiteralPath $systemdManifestPath -Raw|ConvertFrom-Json
$nginxManifest=Get-Content -LiteralPath $nginxManifestPath -Raw|ConvertFrom-Json
if($systemdManifest.schema-ne"xhub-v3-6-systemd-generation-1"-or$systemdManifest.status-ne"SYSTEMD_UNITS_GENERATED_NOT_INSTALLED"-or$systemdManifest.secrets_embedded-ne$false-or$systemdManifest.installed-ne$false-or$systemdManifest.services_started-ne$false-or$systemdManifest.production_approved-ne$false-or$systemdManifest.production_broadcast-ne$false){throw "Invalid systemd generation manifest"}
if($nginxManifest.schema-ne"xhub-v3-6-nginx-generation-1"-or$nginxManifest.status-ne"NGINX_CONFIGS_GENERATED_NOT_INSTALLED"-or$nginxManifest.secrets_embedded-ne$false-or$nginxManifest.installed-ne$false-or$nginxManifest.nginx_reloaded-ne$false-or$nginxManifest.production_approved-ne$false-or$nginxManifest.production_broadcast-ne$false){throw "Invalid Nginx generation manifest"}
if([string]$systemdManifest.runtime_sha256-ine(Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash-or[string]$nginxManifest.tls_profile_sha256-ine(Get-FileHash -LiteralPath $tls -Algorithm SHA256).Hash){throw "Generated configuration source hash mismatch"}
function Test-Files([string]$Directory,$Manifest,[string]$Extension,[string[]]$Required){
    $declared=@($Manifest.files);if($declared.Count-ne 3){throw "Generated manifest must declare three files"}
    $names=@()
    foreach($entry in $declared){
        $name=[string]$entry.file
        if($name-ne[System.IO.Path]::GetFileName($name)-or-not$name.EndsWith($Extension)){throw "Unsafe generated file name"}
        $path=Join-Path $Directory $name
        if(-not(Test-Path -LiteralPath $path -PathType Leaf)-or[string]$entry.sha256-ine(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash){throw "Generated file hash mismatch: $name"}
        $content=Get-Content -LiteralPath $path -Raw
        foreach($value in $Required){if(-not$content.Contains($value)){throw "Generated file is missing required hardening: $value"}}
        foreach($forbidden in @("push_tx","production_broadcast=true","REPLACE_WITH_","BEGIN PRIVATE KEY","Authorization: Bearer")){if($content.Contains($forbidden)){throw "Generated file contains prohibited material: $forbidden"}}
        $names+=$name.ToLowerInvariant()
    }
    if(($names|Select-Object -Unique).Count-ne 3){throw "Generated file names must be unique"}
    $actual=@(Get-ChildItem -LiteralPath $Directory -File|Where-Object{$_.Name-notlike '*manifest.json'})
    if($actual.Count-ne 3){throw "Generated directory contains undeclared files"}
}
Test-Files $systemd $systemdManifest ".service" @("NoNewPrivileges=true","ProtectSystem=strict","CapabilityBoundingSet=","XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE")
Test-Files $nginx $nginxManifest ".conf" @("ssl_protocols TLSv1.3","ssl_verify_client on","proxy_pass http://127.0.0.1:","limit_req zone=")
[pscustomobject]@{schema="xhub-v3-6-deployment-config-validation-1";protocol_version="0x0360";systemd_unit_count=3;nginx_config_count=3;secrets_embedded=$false;installed=$false;services_started=$false;nginx_reloaded=$false;production_approved=$false;production_broadcast=$false;status="DEPLOYMENT_CONFIGS_VERIFIED_NOT_INSTALLED"}|ConvertTo-Json -Depth 4
