$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-runtime.ps1"

function New-Fixtures {
    $identities = @(); $tlsNodes = @(); $runtimeNodes = @()
    for ($index=0; $index -lt 3; $index++) {
        $suffix=[char]([int][char]'a'+$index); $id="custody-$suffix"; $nodeHost="wt-$suffix.example.com"
        $identities += [ordered]@{attester_id=$id;failure_domain="domain-$suffix";operator_id="operator-$suffix";infrastructure_provider="provider-$suffix";region="region-$suffix";attester_public_key=((@("{0:x2}" -f (0x81+$index))*48)-join"");api_base_url="https://$nodeHost";tls_certificate_sha256=((@("{0:x2}" -f (0x21+$index))*32)-join"")}
        $tlsNodes += [ordered]@{attester_id=$id;public_base_url="https://$nodeHost";upstream_url="http://127.0.0.1:8738";server_certificate_file="/etc/nginx/xhub/server.crt";server_private_key_file="/etc/nginx/xhub/server.key";trusted_client_ca_file="/etc/nginx/xhub/client-ca.crt";server_certificate_sha256=((@("{0:x2}" -f (0x21+$index))*32)-join"");minimum_tls_version="TLS1.3";client_certificate_mode="require_and_verify";hsts_max_age_seconds=31536000;rate_limit_requests_per_minute=120;max_request_body_bytes=1048576}
        $runtimeNodes += [ordered]@{attester_id=$id;deployment_host=$nodeHost;service_account="xhub-watchtower";executable_path="/opt/xhub/v3.6/bin/watchtower-v3-6";working_directory="/opt/xhub/v3.6";listen="127.0.0.1:8738";database_path="/var/lib/xhub-watchtower/watchtower-v3_6.sqlite3";api_token_file="/etc/xhub-watchtower/secrets/api-token.txt";confirmers_file="/etc/xhub-watchtower/confirmers.mainnet.json";custody_attesters_file="/etc/xhub-watchtower/custody-attesters.mainnet.json";backup_directory="/var/backups/xhub-watchtower";memory_max_mb=512;limit_nofile=4096;restart_delay_seconds=5;stop_timeout_seconds=30;no_new_privileges=$true;private_tmp=$true;protect_system="strict";protect_home=$true}
    }
    [pscustomobject]@{
        Identities=[ordered]@{schema="xhub-v3-6-watchtower-identities-1";protocol_version="0x0360";network="mainnet";merchant_receipt_public_key=("a8"*48);custody_attestation_threshold=2;custody_attestation_participants=3;attesters=$identities;production_approved=$false}
        Tls=[ordered]@{schema="xhub-v3-6-watchtower-tls-profile-1";protocol_version="0x0360";nodes=$tlsNodes;production_approved=$false}
        Runtime=[ordered]@{schema="xhub-v3-6-watchtower-runtime-1";protocol_version="0x0360";platform="linux-systemd-nginx";executable_sha256=("71"*32);nodes=$runtimeNodes;production_approved=$false;production_broadcast=$false}
    }
}

function Invoke-Validator($fixtures) {
    $runtimePath=Join-Path $env:TEMP ("xhub-runtime-"+[guid]::NewGuid()+".json")
    $identityPath=Join-Path $env:TEMP ("xhub-runtime-identities-"+[guid]::NewGuid()+".json")
    $tlsPath=Join-Path $env:TEMP ("xhub-runtime-tls-"+[guid]::NewGuid()+".json")
    try {
        $fixtures.Runtime|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $runtimePath -Encoding utf8
        $fixtures.Identities|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $identityPath -Encoding utf8
        $fixtures.Tls|ConvertTo-Json -Depth 10|Set-Content -LiteralPath $tlsPath -Encoding utf8
        $previous=$ErrorActionPreference;$ErrorActionPreference="Continue"
        $output=& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -RuntimePath $runtimePath -IdentityManifestPath $identityPath -TlsProfilePath $tlsPath 2>&1
        $exitCode=$LASTEXITCODE;$ErrorActionPreference=$previous
        [pscustomobject]@{ExitCode=$exitCode;Output=$output}
    } finally {Remove-Item -LiteralPath $runtimePath,$identityPath,$tlsPath -Force -ErrorAction SilentlyContinue}
}

$valid=Invoke-Validator (New-Fixtures)
if($valid.ExitCode-ne 0-or($valid.Output|ConvertFrom-Json).status-ne"WATCHTOWER_RUNTIME_PLAN_VALIDATED_ONLY"){throw "Valid runtime rejected: $($valid.Output)"}
$mutations=@(
    {param($f)$f.Runtime.nodes[0].service_account="root"},
    {param($f)$f.Runtime.nodes[0].listen="0.0.0.0:8738"},
    {param($f)$f.Runtime.nodes[0].database_path="relative.sqlite3"},
    {param($f)$f.Runtime.nodes[0].deployment_host="other.example.com"},
    {param($f)$f.Runtime.nodes[0].no_new_privileges=$false},
    {param($f)$f.Runtime.executable_sha256="00"},
    {param($f)$f.Runtime.production_broadcast=$true}
)
foreach($mutation in $mutations){$fixtures=New-Fixtures;&$mutation $fixtures;if((Invoke-Validator $fixtures).ExitCode-eq 0){throw "Invalid runtime accepted"}}
Write-Output "WATCHTOWER_RUNTIME_TESTS_OK"
