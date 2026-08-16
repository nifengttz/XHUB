param(
    [Parameter(Mandatory = $true)][string]$ConfigPath
)

. (Join-Path $PSScriptRoot "common.ps1")
$deployment = Read-DeploymentConfig $ConfigPath
$config = $deployment.Config
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent

Assert-DeploymentConfig $config

$dataDirectory = Resolve-ConfigPath $deployment.Directory "./data"
$logDirectory = Resolve-ConfigPath $deployment.Directory "./logs"
New-Item -ItemType Directory -Force -Path $dataDirectory,$logDirectory | Out-Null

$hubToken = Resolve-ConfigPath $deployment.Directory $config.hub_api_token_file
$watchtowerToken = Resolve-ConfigPath $deployment.Directory $config.watchtower_api_token_file
$hubSecret = Resolve-ConfigPath $deployment.Directory $config.hub_bls_secret_file
[void](Read-SecretFile $hubToken)
[void](Read-SecretFile $watchtowerToken)
[void](Read-SecretFile $hubSecret)

$watchtowerEnv = @{
    XHUB_WATCHTOWER_LISTEN = $config.watchtower_listen
    XHUB_WATCHTOWER_DB = Resolve-ConfigPath $deployment.Directory $config.watchtower_db
    XHUB_WATCHTOWER_API_TOKEN_FILE = $watchtowerToken
    XHUB_WATCHTOWER_CONFIRMERS_FILE = Resolve-ConfigPath $deployment.Directory $config.watchtower_confirmers_file
}
$custodyAttesters = Get-OptionalConfigValue $config "watchtower_custody_attesters_file"
if ($custodyAttesters) {
    $watchtowerEnv.XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE = Resolve-ConfigPath $deployment.Directory $custodyAttesters
}
$hubEnv = @{
    XHUB_HUB_LISTEN = $config.hub_listen
    XHUB_HUB_DB = Resolve-ConfigPath $deployment.Directory $config.hub_db
    XHUB_HUB_BLS_SECRET_FILE = $hubSecret
    XHUB_HUB_API_TOKEN_FILE = $hubToken
    XHUB_WATCHTOWER_API_TOKEN_FILE = $watchtowerToken
    XHUB_WATCHTOWER_URL = $config.watchtower_url
    XHUB_WATCHTOWER_RECIPIENT_ID = $config.watchtower_recipient_id
    XHUB_CHIA_RPC_URL = $config.chia_rpc_url
}
$rpcCert = Get-OptionalConfigValue $config "chia_rpc_cert_file"
$rpcKey = Get-OptionalConfigValue $config "chia_rpc_key_file"
if (($null -eq $rpcCert) -ne ($null -eq $rpcKey)) { throw "RPC certificate and key must be configured together" }
if ($rpcCert) {
    $hubEnv.XHUB_CHIA_RPC_CERT_FILE = Resolve-ConfigPath $deployment.Directory $rpcCert
    $hubEnv.XHUB_CHIA_RPC_KEY_FILE = Resolve-ConfigPath $deployment.Directory $rpcKey
}
$walletEnv = @{ XHUB_WALLET_LISTEN = $config.wallet_listen }
$cargoPath = (Get-Command cargo -ErrorAction Stop).Source

& $cargoPath build --offline --manifest-path (Join-Path $root "watchtower-v3_6/Cargo.toml") --bin watchtower-v3-6
if ($LASTEXITCODE -ne 0) { throw "Watchtower build failed" }
& $cargoPath build --offline --manifest-path (Join-Path $root "hub-v3_6/Cargo.toml") --bin hub-v3-6
if ($LASTEXITCODE -ne 0) { throw "HUB build failed" }
& $cargoPath build --offline --manifest-path (Join-Path $root "wallet-v3_6/Cargo.toml") --bin wallet-v3-6
if ($LASTEXITCODE -ne 0) { throw "Wallet build failed" }

function Start-XhubProcess([string]$Name, [string]$Executable, [hashtable]$Environment) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $true
    $start.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
    $previous = @{}
    try {
        foreach ($entry in $Environment.GetEnumerator()) {
            $previous[$entry.Key] = [System.Environment]::GetEnvironmentVariable($entry.Key, "Process")
            [System.Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, "Process")
        }
        $process = [System.Diagnostics.Process]::Start($start)
    } finally {
        foreach ($entry in $Environment.GetEnumerator()) {
            [System.Environment]::SetEnvironmentVariable($entry.Key, $previous[$entry.Key], "Process")
        }
    }
    [pscustomobject]@{ Name = $Name; Pid = $process.Id; Executable = $Executable }
}

$processes = @(
    Start-XhubProcess "watchtower" (Join-Path $root "watchtower-v3_6/target/debug/watchtower-v3-6.exe") $watchtowerEnv
    Start-XhubProcess "hub" (Join-Path $root "hub-v3_6/target/debug/hub-v3-6.exe") $hubEnv
    Start-XhubProcess "wallet" (Join-Path $root "wallet-v3_6/target/debug/wallet-v3-6.exe") $walletEnv
)
$pidPath = Join-Path $dataDirectory "processes.json"
$processes | ConvertTo-Json | Set-Content -LiteralPath $pidPath -Encoding utf8
$processes | Format-Table -AutoSize
Write-Host "Process manifest: $pidPath"
