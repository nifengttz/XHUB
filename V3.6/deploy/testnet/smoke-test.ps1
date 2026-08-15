param(
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [string]$FundingRegistrationPath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-DeploymentConfig $ConfigPath
$config = $deployment.Config
Assert-DeploymentConfig $config
$hubToken = Read-SecretFile (Resolve-ConfigPath $deployment.Directory $config.hub_api_token_file)
$watchtowerToken = Read-SecretFile (Resolve-ConfigPath $deployment.Directory $config.watchtower_api_token_file)
$protocolHeaders = @{ "x-xhub-protocol-version" = "0x0360" }

function BaseUrl([string]$Listen) { "http://$Listen" }
function AuthHeaders([string]$Token) {
    @{ "x-xhub-protocol-version" = "0x0360"; "Authorization" = "Bearer $Token" }
}

$wallet = Invoke-RestMethod -Uri "$((BaseUrl $config.wallet_listen))/api/v3.6/health" -TimeoutSec 10
$hub = Invoke-RestMethod -Uri "$((BaseUrl $config.hub_listen))/api/v3.6/health" -Headers (AuthHeaders $hubToken) -TimeoutSec 10
$watchtower = Invoke-RestMethod -Uri "$((BaseUrl $config.watchtower_listen))/api/v3.6/health" -Headers (AuthHeaders $watchtowerToken) -TimeoutSec 10

try {
    Invoke-WebRequest -Uri "$((BaseUrl $config.hub_listen))/api/v3.6/health" -TimeoutSec 10 | Out-Null
    throw "HUB accepted a request without authentication"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw }
}
try {
    Invoke-WebRequest -Uri "$((BaseUrl $config.watchtower_listen))/api/v3.6/health" -TimeoutSec 10 | Out-Null
    throw "Watchtower accepted a request without authentication"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw }
}

$registration = $null
if ($FundingRegistrationPath) {
    $body = Get-Content -LiteralPath $FundingRegistrationPath -Raw
    $registration = Invoke-RestMethod -Method Post -Uri "$((BaseUrl $config.hub_listen))/api/v3.6/funding-coins" -Headers (AuthHeaders $hubToken) -ContentType "application/json" -Body $body -TimeoutSec 30
}

[pscustomobject]@{
    schema = "xhub-v3-6-testnet-smoke-1"
    wallet = $wallet.status
    hub = $hub.status
    watchtower = $watchtower.status
    unauthenticated_requests_rejected = $true
    funding_registration = $registration
} | ConvertTo-Json -Depth 8
