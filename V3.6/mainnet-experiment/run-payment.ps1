param(
    [string]$PaymentRequest = (Join-Path $PSScriptRoot "payment-1-mojo.json"),
    [string]$ResultFile = (Join-Path $PSScriptRoot "payment-1-mojo-result.json")
)

$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
$walletManifest = Join-Path $root "wallet-v3_6\Cargo.toml"
$hubRequestFile = Join-Path $PSScriptRoot "private\payment-1-mojo-hub-request.json"
$hubPackageFile = Join-Path $PSScriptRoot "private\payment-1-mojo-hub-package.json"
$confirmationFile = Join-Path $PSScriptRoot "private\payment-1-mojo-confirmation.json"
$merchantSecretFile = Join-Path $root "local-secrets\mainnet-experiment-merchant-receipt-bls.hex"
$hubTokenFile = Join-Path $root "local-secrets\mainnet-experiment-hub-api-token.txt"
$watchtowerTokenFile = Join-Path $root "local-secrets\mainnet-experiment-watchtower-api-token.txt"
$hubBase = "http://127.0.0.1:8737"
$watchtowerBase = "http://127.0.0.1:8738"
$fundingCoinId = "d8d089881dde12de0bdb8a078df9ab047da307d3f671b5b188b78448d570ea9d"
$recipientId = "watchtower-mainnet-experiment-1"
$deliveryKey = "mainnet-experiment-$fundingCoinId-state-1-watchtower-1"

function Read-Secret([string]$Path) {
    $value = (Get-Content -Raw -LiteralPath $Path).Trim()
    if ($value.Length -lt 32) {
        throw "Secret file is missing or invalid: $Path"
    }
    return $value
}

function Write-Json([string]$Path, $Value) {
    $directory = Split-Path $Path -Parent
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $json = $Value | ConvertTo-Json -Depth 20
    $utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $json + [Environment]::NewLine, $utf8WithoutBom)
}

function Invoke-VersionedJson(
    [string]$Method,
    [string]$Uri,
    [string]$Token,
    $Body = $null
) {
    $headers = @{
        Authorization = "Bearer $Token"
        "x-xhub-protocol-version" = "0x0360"
    }
    $parameters = @{
        Method = $Method
        Uri = $Uri
        Headers = $headers
        UseBasicParsing = $true
    }
    if ($null -ne $Body) {
        $parameters.ContentType = "application/json"
        $parameters.Body = ($Body | ConvertTo-Json -Depth 20 -Compress)
    }
    return Invoke-RestMethod @parameters
}

if (-not (Test-Path -LiteralPath $PaymentRequest)) {
    throw "Payment request not found: $PaymentRequest"
}

$payment = Get-Content -Raw -LiteralPath $PaymentRequest | ConvertFrom-Json
if ($payment.schema -ne "xhub-v3-6-mainnet-payment-request-1" -or
    $payment.protocol_version -ne "0x0360" -or
    $payment.network -ne "mainnet" -or
    $payment.funding_coin_id -ne $fundingCoinId -or
    $payment.amount -ne "1" -or
    [string]::IsNullOrWhiteSpace($payment.user_authorization_signature)) {
    throw "A signed, exact 1 mojo V3.6 mainnet experiment request is required"
}

cargo run --quiet --manifest-path $walletManifest --bin mainnet-payment -- `
    verify-payment $PaymentRequest $hubRequestFile
if ($LASTEXITCODE -ne 0) {
    throw "Independent V3.6 payment verification failed"
}

$hubToken = Read-Secret $hubTokenFile
$watchtowerToken = Read-Secret $watchtowerTokenFile
$hubRequest = Get-Content -Raw -LiteralPath $hubRequestFile | ConvertFrom-Json

try {
    $reservation = Invoke-VersionedJson POST "$hubBase/api/v3.6/reservations" $hubToken $hubRequest
} catch {
    $nonce = $hubRequest.reservation_nonce
    $reservation = Invoke-VersionedJson GET "$hubBase/api/v3.6/funding-coins/$fundingCoinId/reservations/$nonce`?protocol_version=0x0360" $hubToken
}
if ($reservation.status -ne "SIGNED" -or $reservation.ledger_written -ne $true) {
    throw "HUB did not return a persisted SIGNED reservation"
}

$stateSequence = [uint64]$reservation.state_sequence
$hubPackage = Invoke-VersionedJson GET "$hubBase/api/v3.6/funding-coins/$fundingCoinId/recovery-packages/$stateSequence`?protocol_version=0x0360" $hubToken
Write-Json $hubPackageFile $hubPackage

$deliveryBody = @{
    protocol_version = "0x0360"
    recipient_id = $recipientId
    recipient_kind = "WATCHTOWER"
    idempotency_key = $deliveryKey
}
$delivery = Invoke-VersionedJson POST "$hubBase/api/v3.6/funding-coins/$fundingCoinId/recovery-packages/$stateSequence/deliveries" $hubToken $deliveryBody
if ($delivery.delivery.status -ne "DELIVERED") {
    throw "RecoveryPackage delivery did not reach DELIVERED"
}

$watchtowerPackage = Invoke-VersionedJson GET "$watchtowerBase/api/v3.6/funding-coins/$fundingCoinId/recovery-packages/$stateSequence`?protocol_version=0x0360" $watchtowerToken
if ($watchtowerPackage.recovery_package_content_hash -ne $hubPackage.recovery_package_content_hash -or
    $watchtowerPackage.recovery_package_canonical_hex -ne $hubPackage.recovery_package_canonical_hex) {
    throw "Watchtower RecoveryPackage does not match the HUB package"
}

cargo run --quiet --manifest-path $walletManifest --bin mainnet-payment -- `
    confirm-delivery $hubPackageFile $confirmationFile $merchantSecretFile
if ($LASTEXITCODE -ne 0) {
    throw "Merchant DeliveryConfirmation signing failed"
}
$confirmation = Get-Content -Raw -LiteralPath $confirmationFile | ConvertFrom-Json
$confirmationResult = Invoke-VersionedJson POST "$watchtowerBase/api/v3.6/delivery-confirmations" $watchtowerToken $confirmation

$greenlight = Invoke-VersionedJson GET "$watchtowerBase/api/v3.6/funding-coins/$fundingCoinId/states/$stateSequence/entries/0/greenlight?protocol_version=0x0360&threshold=1" $watchtowerToken
if ($greenlight.delivered -ne $true -or
    [uint64]$greenlight.signer_count -ne 1 -or
    [uint64]$greenlight.failure_domain_count -ne 1) {
    throw "Watchtower greenlight threshold was not satisfied"
}

$publicResult = [ordered]@{
    schema = "xhub-v3-6-mainnet-payment-result-1"
    protocol_version = "0x0360"
    network = "mainnet"
    mainnet_approved = $false
    chain_broadcast = $false
    funding_coin_id = $fundingCoinId
    amount = "1"
    reservation_nonce = $hubRequest.reservation_nonce
    authorization_hash = $reservation.authorization_hash
    state_sequence = $stateSequence
    checkpoint_hash = $reservation.checkpoint_hash
    recovery_package_content_hash = $hubPackage.recovery_package_content_hash
    hub_status = $reservation.status
    ledger_written = $reservation.ledger_written
    delivery_status = $delivery.delivery.status
    delivery_attempt_count = $delivery.delivery.attempt_count
    confirmation_status = $confirmationResult.status
    greenlight = $greenlight
    completed_at_utc = [DateTime]::UtcNow.ToString("o")
}
Write-Json $ResultFile $publicResult

Write-Host "Off-chain reservation: SIGNED"
Write-Host "RecoveryPackage delivery: DELIVERED"
Write-Host "Greenlight: delivered=true, signers=1, failure_domains=1"
Write-Host "Result: $ResultFile"
