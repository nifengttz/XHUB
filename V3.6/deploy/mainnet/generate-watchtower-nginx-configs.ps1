param(
    [Parameter(Mandatory = $true)][string]$TlsProfilePath,
    [Parameter(Mandatory = $true)][string]$IdentityManifestPath,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = "Stop"
$validator = Join-Path $PSScriptRoot "validate-watchtower-tls-profile.ps1"
$validation = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $validator `
    -TlsProfilePath $TlsProfilePath -IdentityManifestPath $IdentityManifestPath | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $validation.status -ne "TLS_PROFILE_VALIDATED_CONFIG_ONLY") { throw "Watchtower TLS profile validation failed" }
if (Test-Path -LiteralPath $OutputDirectory) { throw "Output directory already exists: $OutputDirectory" }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$tlsResolved = (Resolve-Path -LiteralPath $TlsProfilePath).Path
$profile = Get-Content -LiteralPath $tlsResolved -Raw | ConvertFrom-Json
$generated = @()
$template = @'
limit_req_zone $binary_remote_addr zone={{ZONE}}:10m rate={{RATE}}r/m;

server {
    listen {{PORT}} ssl;
    server_name {{HOST}};

    ssl_protocols TLSv1.3;
    ssl_certificate {{CERT}};
    ssl_certificate_key {{KEY}};
    ssl_client_certificate {{CLIENT_CA}};
    ssl_verify_client on;
    ssl_verify_depth 2;
    ssl_session_tickets off;
    ssl_stapling on;
    ssl_stapling_verify on;

    add_header Strict-Transport-Security "max-age={{HSTS}}; includeSubDomains" always;
    client_max_body_size {{MAX_BODY}};

    location / {
        limit_req zone={{ZONE}} burst=10 nodelay;
        proxy_pass {{UPSTREAM}};
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Authorization $http_authorization;
        proxy_request_buffering on;
        proxy_buffering on;
        proxy_connect_timeout 5s;
        proxy_read_timeout 30s;
        proxy_send_timeout 30s;
    }
}
'@
foreach ($node in @($profile.nodes)) {
    $id=([string]$node.attester_id).ToLowerInvariant()
    if($id -notmatch '^[a-z0-9][a-z0-9_-]{0,63}$'){throw "attester_id is unsafe for an Nginx zone name"}
    foreach($field in @("server_certificate_file","server_private_key_file","trusted_client_ca_file")){
        $value=[string]$node.$field
        if($value -notmatch '^/[A-Za-z0-9._/-]+$' -or $value.Contains("//") -or $value.Split('/') -contains '..'){throw "$field must be an absolute POSIX path"}
    }
    $uri=[Uri]$node.public_base_url
    $port=if($uri.IsDefaultPort){443}else{$uri.Port}
    $zone=("xhub_wt_"+$id.Replace('-','_'))
    $content=$template.Replace("{{ZONE}}",$zone).Replace("{{RATE}}",[string]$node.rate_limit_requests_per_minute).Replace("{{PORT}}",[string]$port).Replace("{{HOST}}",$uri.DnsSafeHost).Replace("{{CERT}}",[string]$node.server_certificate_file).Replace("{{KEY}}",[string]$node.server_private_key_file).Replace("{{CLIENT_CA}}",[string]$node.trusted_client_ca_file).Replace("{{HSTS}}",[string]$node.hsts_max_age_seconds).Replace("{{MAX_BODY}}",[string]$node.max_request_body_bytes).Replace("{{UPSTREAM}}",([string]$node.upstream_url).TrimEnd('/'))
    $fileName="xhub-watchtower-v3-6-$id.conf";$path=Join-Path $OutputDirectory $fileName
    [System.IO.File]::WriteAllText($path,($content.TrimStart()+[Environment]::NewLine),[System.Text.UTF8Encoding]::new($false))
    $generated += [ordered]@{file=$fileName;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant();certificate_sha256=([string]$node.server_certificate_sha256).ToLowerInvariant()}
}
$manifest=[ordered]@{schema="xhub-v3-6-nginx-generation-1";protocol_version="0x0360";tls_profile_sha256=(Get-FileHash -LiteralPath $tlsResolved -Algorithm SHA256).Hash.ToLowerInvariant();files=$generated;secrets_embedded=$false;installed=$false;nginx_reloaded=$false;production_approved=$false;production_broadcast=$false;status="NGINX_CONFIGS_GENERATED_NOT_INSTALLED"}
[System.IO.File]::WriteAllText((Join-Path $OutputDirectory "nginx-manifest.json"),(($manifest|ConvertTo-Json -Depth 6)+[Environment]::NewLine),[System.Text.UTF8Encoding]::new($false))
Write-Output "Generated $OutputDirectory"
