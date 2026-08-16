$ErrorActionPreference="Stop"
$systemdGenerator=Join-Path $PSScriptRoot "generate-watchtower-systemd-units.ps1"
$nginxGenerator=Join-Path $PSScriptRoot "generate-watchtower-nginx-configs.ps1"
$generatedValidator=Join-Path $PSScriptRoot "verify-watchtower-generated-configs.ps1"
$generatorSource=(Get-Content -LiteralPath $systemdGenerator,$nginxGenerator -Raw)-join"`n"
foreach($forbiddenCommand in @("systemctl","nginx -s","Start-Process","Invoke-Expression"," ssh "," scp ")){if($generatorSource.Contains($forbiddenCommand)){throw "Deployment generator may mutate external state: $forbiddenCommand"}}
$base=Join-Path $env:TEMP ("xhub-deployment-generation-"+[guid]::NewGuid())
New-Item -ItemType Directory -Path $base|Out-Null
try {
    $identities=@();$tlsNodes=@();$runtimeNodes=@()
    for($index=0;$index-lt 3;$index++){
        $suffix=[char]([int][char]'a'+$index);$id="custody-$suffix";$nodeHost="wt-$suffix.example.com";$pin=((@("{0:x2}"-f(0x21+$index))*32)-join"")
        $identities += [ordered]@{attester_id=$id;failure_domain="domain-$suffix";operator_id="operator-$suffix";infrastructure_provider="provider-$suffix";region="region-$suffix";attester_public_key=((@("{0:x2}"-f(0x81+$index))*48)-join"");api_base_url="https://$nodeHost";tls_certificate_sha256=$pin}
        $tlsNodes += [ordered]@{attester_id=$id;public_base_url="https://$nodeHost";upstream_url="http://127.0.0.1:8738";server_certificate_file="/etc/nginx/xhub/server.crt";server_private_key_file="/etc/nginx/xhub/server.key";trusted_client_ca_file="/etc/nginx/xhub/client-ca.crt";server_certificate_sha256=$pin;minimum_tls_version="TLS1.3";client_certificate_mode="require_and_verify";hsts_max_age_seconds=31536000;rate_limit_requests_per_minute=120;max_request_body_bytes=1048576}
        $runtimeNodes += [ordered]@{attester_id=$id;deployment_host=$nodeHost;service_account="xhub-watchtower";executable_path="/opt/xhub/v3.6/bin/watchtower-v3-6";working_directory="/opt/xhub/v3.6";listen="127.0.0.1:8738";database_path="/var/lib/xhub-watchtower/watchtower-v3_6.sqlite3";api_token_file="/etc/xhub-watchtower/secrets/api-token.txt";confirmers_file="/etc/xhub-watchtower/confirmers.mainnet.json";custody_attesters_file="/etc/xhub-watchtower/custody-attesters.mainnet.json";backup_directory="/var/backups/xhub-watchtower";memory_max_mb=512;limit_nofile=4096;restart_delay_seconds=5;stop_timeout_seconds=30;no_new_privileges=$true;private_tmp=$true;protect_system="strict";protect_home=$true}
    }
    $identity=[ordered]@{schema="xhub-v3-6-watchtower-identities-1";protocol_version="0x0360";network="mainnet";merchant_receipt_public_key=("a8"*48);custody_attestation_threshold=2;custody_attestation_participants=3;attesters=$identities;production_approved=$false}
    $tls=[ordered]@{schema="xhub-v3-6-watchtower-tls-profile-1";protocol_version="0x0360";nodes=$tlsNodes;production_approved=$false}
    $runtime=[ordered]@{schema="xhub-v3-6-watchtower-runtime-1";protocol_version="0x0360";platform="linux-systemd-nginx";executable_sha256=("71"*32);nodes=$runtimeNodes;production_approved=$false;production_broadcast=$false}
    $identityPath=Join-Path $base "identities.json";$tlsPath=Join-Path $base "tls.json";$runtimePath=Join-Path $base "runtime.json"
    $identity|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $identityPath -Encoding utf8
    $tls|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $tlsPath -Encoding utf8
    $runtime|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $runtimePath -Encoding utf8
    $systemdOut=Join-Path $base "systemd";$nginxOut=Join-Path $base "nginx"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $systemdGenerator -RuntimePath $runtimePath -IdentityManifestPath $identityPath -TlsProfilePath $tlsPath -OutputDirectory $systemdOut *> $null
    if($LASTEXITCODE-ne 0){throw "systemd generation failed"}
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $nginxGenerator -TlsProfilePath $tlsPath -IdentityManifestPath $identityPath -OutputDirectory $nginxOut *> $null
    if($LASTEXITCODE-ne 0){throw "Nginx generation failed"}
    $systemdManifest=Get-Content -LiteralPath (Join-Path $systemdOut "systemd-manifest.json") -Raw|ConvertFrom-Json
    $nginxManifest=Get-Content -LiteralPath (Join-Path $nginxOut "nginx-manifest.json") -Raw|ConvertFrom-Json
    if($systemdManifest.status-ne"SYSTEMD_UNITS_GENERATED_NOT_INSTALLED"-or@($systemdManifest.files).Count-ne 3-or$systemdManifest.services_started-ne$false){throw "systemd manifest invalid"}
    if($nginxManifest.status-ne"NGINX_CONFIGS_GENERATED_NOT_INSTALLED"-or@($nginxManifest.files).Count-ne 3-or$nginxManifest.nginx_reloaded-ne$false){throw "Nginx manifest invalid"}
    $unit=Get-Content -LiteralPath (Join-Path $systemdOut $systemdManifest.files[0].file) -Raw
    foreach($required in @("NoNewPrivileges=true","ProtectSystem=strict","CapabilityBoundingSet=","XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE","127.0.0.1:8738")){if(-not$unit.Contains($required)){throw "systemd hardening missing: $required"}}
    $nginx=Get-Content -LiteralPath (Join-Path $nginxOut $nginxManifest.files[0].file) -Raw
    foreach($required in @("ssl_protocols TLSv1.3","ssl_verify_client on","limit_req zone=","proxy_pass http://127.0.0.1:8738")){if(-not$nginx.Contains($required)){throw "Nginx hardening missing: $required"}}
    $allGenerated=(Get-ChildItem -LiteralPath $systemdOut,$nginxOut -File|Get-Content -Raw)-join"`n"
    foreach($forbidden in @("push_tx","production_broadcast=true","REPLACE_WITH_","BEGIN PRIVATE KEY")){if($allGenerated.Contains($forbidden)){throw "Generated deployment contains prohibited material: $forbidden"}}
    $verified=& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $generatedValidator -SystemdDirectory $systemdOut -NginxDirectory $nginxOut -RuntimePath $runtimePath -TlsProfilePath $tlsPath|ConvertFrom-Json
    if($LASTEXITCODE-ne 0-or$verified.status-ne"DEPLOYMENT_CONFIGS_VERIFIED_NOT_INSTALLED"){throw "Generated deployment verification failed"}
} finally {
    Remove-Item -LiteralPath $base -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output "WATCHTOWER_DEPLOYMENT_GENERATION_TESTS_OK"
