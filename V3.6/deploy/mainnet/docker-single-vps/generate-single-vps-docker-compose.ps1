param(
    [Parameter(Mandatory = $true)][string]$ProfilePath,
    [Parameter(Mandatory = $true)][string]$CustodyAttestersPath,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference="Stop"
$validator=Join-Path $PSScriptRoot "validate-single-vps-docker-profile.ps1"
$validation=& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator -ProfilePath $ProfilePath -CustodyAttestersPath $CustodyAttestersPath|ConvertFrom-Json
if($LASTEXITCODE-ne0-or$validation.status-ne"SINGLE_VPS_DOCKER_PLAN_VALIDATED_TEST_ONLY"){throw "Single VPS Docker profile validation failed"}
if(Test-Path -LiteralPath $OutputDirectory){throw "Output directory already exists: $OutputDirectory"}
New-Item -ItemType Directory -Path $OutputDirectory|Out-Null
$profileResolved=(Resolve-Path -LiteralPath $ProfilePath).Path
$attestersResolved=(Resolve-Path -LiteralPath $CustodyAttestersPath).Path
$profile=Get-Content -LiteralPath $profileResolved -Raw|ConvertFrom-Json
$lines=@("services:")
foreach($instance in @($profile.instances)){
    $id=[string]$instance.attester_id;$port=[int64]$instance.listen_port
    $lines += @(
        "  ${id}:",
        "    image: $($profile.image)",
        "    container_name: xhub-v36-${id}",
        "    network_mode: host",
        "    restart: unless-stopped",
        "    user: `"65532:65532`"",
        "    read_only: true",
        "    cap_drop:",
        "      - ALL",
        "    security_opt:",
        "      - no-new-privileges:true",
        "    tmpfs:",
        "      - /tmp:rw,noexec,nosuid,size=16m",
        "    environment:",
        "      XHUB_WATCHTOWER_LISTEN: 127.0.0.1:${port}",
        "      XHUB_WATCHTOWER_DB: /var/lib/xhub-watchtower/watchtower-v3_6.sqlite3",
        "      XHUB_WATCHTOWER_API_TOKEN_FILE: /run/secrets/api-token",
        "      XHUB_WATCHTOWER_CONFIRMERS_FILE: /run/config/confirmers.json",
        "      XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE: /run/config/custody-attesters.json",
        "    volumes:",
        "      - $($instance.database_directory):/var/lib/xhub-watchtower",
        "      - $($instance.api_token_file):/run/secrets/api-token:ro",
        "      - $($profile.confirmers_file):/run/config/confirmers.json:ro",
        "      - $($profile.custody_attesters_file):/run/config/custody-attesters.json:ro",
        "    mem_limit: 512m",
        "    pids_limit: 128",
        "    healthcheck:",
        "      test: [`"CMD-SHELL`", `"TOKEN=`$`$(cat /run/secrets/api-token); curl --fail --silent -H \`"Authorization: Bearer `$`$TOKEN\`" http://127.0.0.1:${port}/api/v3.6/health >/dev/null`"]",
        "      interval: 30s",
        "      timeout: 5s",
        "      retries: 3",
        "      start_period: 10s"
    )
}
$composePath=Join-Path $OutputDirectory "compose.yaml"
[System.IO.File]::WriteAllText($composePath,(($lines-join[Environment]::NewLine)+[Environment]::NewLine),[System.Text.UTF8Encoding]::new($false))
$manifest=[ordered]@{schema="xhub-v3-6-single-vps-docker-generation-1";protocol_version="0x0360";profile_sha256=(Get-FileHash -LiteralPath $profileResolved -Algorithm SHA256).Hash.ToLowerInvariant();custody_attesters_sha256=(Get-FileHash -LiteralPath $attestersResolved -Algorithm SHA256).Hash.ToLowerInvariant();compose_sha256=(Get-FileHash -LiteralPath $composePath -Algorithm SHA256).Hash.ToLowerInvariant();instance_count=3;failure_domain_count=1;failure_domain_enforced=$false;docker_validation_performed=$false;containers_started=$false;test_only=$true;production_ready=$false;production_broadcast=$false;status="SINGLE_VPS_DOCKER_COMPOSE_GENERATED_NOT_STARTED"}
[System.IO.File]::WriteAllText((Join-Path $OutputDirectory "compose-manifest.json"),(($manifest|ConvertTo-Json -Depth 5)+[Environment]::NewLine),[System.Text.UTF8Encoding]::new($false))
Write-Output "Generated $OutputDirectory"
