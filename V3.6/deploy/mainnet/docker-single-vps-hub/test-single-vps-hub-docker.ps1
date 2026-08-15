$ErrorActionPreference="Stop"
$base=$PSScriptRoot
$docker=Get-Content -LiteralPath (Join-Path $base "Dockerfile") -Raw
$compose=Get-Content -LiteralPath (Join-Path $base "compose.single-vps.example.yaml") -Raw
$watchtowers=@(Get-Content -LiteralPath (Join-Path $base "watchtowers.single-vps.example.json") -Raw|ConvertFrom-Json|ForEach-Object{$_})
foreach($required in @("cargo build --locked --release","--bin hub-v3-6","USER 65532:65532","ENTRYPOINT")){if(-not$docker.Contains($required)){throw "Dockerfile missing $required"}}
foreach($forbidden in @("COPY . .","latest","push_tx","BEGIN PRIVATE KEY")){if($docker.Contains($forbidden)){throw "Dockerfile contains prohibited setting $forbidden"}}
foreach($required in @("network_mode: host","127.0.0.1:18737","read_only: true","no-new-privileges:true","cap_drop:","pids_limit: 128","https://api.coinset.org","XHUB_WATCHTOWERS_FILE",'-H \"Authorization: Bearer $$TOKEN\"','xhub.test-only: "true"','xhub.production-ready: "false"')){if(-not$compose.Contains($required)){throw "Compose missing $required"}}
foreach($forbidden in @("0.0.0.0","ports:","privileged: true",'production-ready: "true"','production-broadcast: "true"',"push_tx")){if($compose.Contains($forbidden)){throw "Compose contains prohibited setting $forbidden"}}
if($watchtowers.Count-ne3){throw "Exactly three Watchtower endpoints are required"}
$ids=@($watchtowers|ForEach-Object{$_.recipient_id});$urls=@($watchtowers|ForEach-Object{$_.base_url});$tokens=@($watchtowers|ForEach-Object{$_.api_token_file})
foreach($values in @($ids,$urls,$tokens)){if(($values|Select-Object -Unique).Count-ne3){throw "Watchtower IDs, URLs and token files must be distinct"}}
for($index=0;$index-lt3;$index++){if($ids[$index]-ne("wt-"+[char]([int][char]'a'+$index))-or$urls[$index]-ne("http://127.0.0.1:"+(18738+$index))){throw "Unexpected Watchtower mapping"}}
Write-Output "SINGLE_VPS_HUB_DOCKER_TESTS_OK"
