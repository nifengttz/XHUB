param(
    [Parameter(Mandatory = $true)][string]$ProfilePath,
    [Parameter(Mandatory = $true)][string]$CustodyAttestersPath
)

$ErrorActionPreference="Stop"
function Assert-ExactFields($Object,[string[]]$Expected,[string]$Context){$actual=@($Object.PSObject.Properties.Name|Sort-Object);$sorted=@($Expected|Sort-Object);if(($actual-join'|')-ne($sorted-join'|')){throw "$Context fields do not match the frozen schema"}}
function Assert-Path([string]$Value,[string]$Field){if($Value-notmatch'^/[A-Za-z0-9._/-]+$'-or$Value.Contains('//')-or$Value.Split('/')-contains'..'-or$Value-match'^/(root|home)(/|$)'){throw "$Field must be a normalized absolute Linux path outside home/root"}}
$profile=Get-Content -LiteralPath (Resolve-Path -LiteralPath $ProfilePath) -Raw|ConvertFrom-Json
$attesters=@(Get-Content -LiteralPath (Resolve-Path -LiteralPath $CustodyAttestersPath) -Raw|ConvertFrom-Json|ForEach-Object{$_})
Assert-ExactFields $profile @("schema","protocol_version","mode","image","failure_domain","custody_threshold","confirmers_file","custody_attesters_file","instances","failure_domain_enforced","test_only","production_ready","production_broadcast") "single VPS profile"
if($profile.schema-ne"xhub-v3-6-single-vps-docker-test-1"-or$profile.protocol_version-ne"0x0360"-or$profile.mode-ne"single-vps-docker-test"){throw "Unsupported single VPS Docker profile"}
if($profile.failure_domain_enforced-ne$false-or$profile.test_only-ne$true-or$profile.production_ready-ne$false-or$profile.production_broadcast-ne$false){throw "Single VPS profile must remain test-only and fail closed for production"}
if([int64]$profile.custody_threshold-ne 2){throw "Single VPS Docker test threshold must be 2"}
if([string]$profile.image-notmatch'^[a-z0-9][a-z0-9._/-]*:[A-Za-z0-9._-]+$'-or[string]$profile.image-match'(?i):latest$'){throw "Docker image must use an explicit non-latest tag"}
if([string]::IsNullOrWhiteSpace([string]$profile.failure_domain)-or[string]$profile.failure_domain-match'^REPLACE_WITH_'){throw "failure_domain must be explicit"}
Assert-Path ([string]$profile.confirmers_file) "confirmers_file";Assert-Path ([string]$profile.custody_attesters_file) "custody_attesters_file"
$instances=@($profile.instances);if($instances.Count-ne 3-or$attesters.Count-ne 3){throw "Exactly three Docker instances and attesters are required"}
$ids=@();$ports=@();$databases=@();$tokens=@()
foreach($instance in $instances){
    Assert-ExactFields $instance @("attester_id","listen_port","database_directory","api_token_file") "Docker instance"
    $id=[string]$instance.attester_id
    if($id-notmatch'^[a-z0-9][a-z0-9_-]{0,31}$'){throw "attester_id is unsafe for a Compose service name"}
    $port=[int64]$instance.listen_port;if($port-lt1024-or$port-gt65535){throw "listen_port must be in 1024..65535"}
    Assert-Path ([string]$instance.database_directory) "database_directory";Assert-Path ([string]$instance.api_token_file) "api_token_file"
    $ids+=$id;$ports+=$port;$databases+=([string]$instance.database_directory).ToLowerInvariant();$tokens+=([string]$instance.api_token_file).ToLowerInvariant()
}
foreach($values in @($ids,$ports,$databases,$tokens)){if(($values|Select-Object -Unique).Count-ne 3){throw "Docker instance IDs, ports, data directories, and API tokens must be distinct"}}
$attesterById=@{}
foreach($attester in $attesters){
    Assert-ExactFields $attester @("signer_id","failure_domain","signer_public_key") "custody attester"
    $id=([string]$attester.signer_id).ToLowerInvariant()
    if($attesterById.ContainsKey($id)-or$id-notin$ids){throw "Custody attester does not map uniquely to a Docker instance"}
    if([string]$attester.failure_domain-cne[string]$profile.failure_domain){throw "All Docker test attesters must share the single VPS failure domain"}
    if([string]$attester.signer_public_key-notmatch'^[0-9a-fA-F]{96}$'){throw "Each custody attester needs a 48-byte BLS public key"}
    $attesterById[$id]=$attester
}
$keys=@($attesters|ForEach-Object{([string]$_.signer_public_key).ToLowerInvariant()});if(($keys|Select-Object -Unique).Count-ne3){throw "Docker test attesters must use three distinct BLS public keys"}
[pscustomobject]@{schema="xhub-v3-6-single-vps-docker-validation-1";protocol_version="0x0360";instance_count=3;custody_threshold=2;distinct_public_key_count=3;failure_domain_count=1;failure_domain_enforced=$false;host_network_required=$true;test_only=$true;production_ready=$false;production_broadcast=$false;status="SINGLE_VPS_DOCKER_PLAN_VALIDATED_TEST_ONLY"}|ConvertTo-Json -Depth 4
