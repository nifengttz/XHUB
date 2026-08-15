param(
    [Parameter(Mandatory = $true)][string]$ParametersPath
)

$ErrorActionPreference = "Stop"
$parameters = Get-Content -LiteralPath (Resolve-Path -LiteralPath $ParametersPath) -Raw | ConvertFrom-Json

if ($parameters.schema -ne "xhub-v3-6-mainnet-parameters-1") { throw "Unsupported mainnet parameters schema" }
if ($parameters.protocol_version -ne "0x0360") { throw "Mainnet parameters must use protocol 0x0360" }
if ([string]::IsNullOrWhiteSpace([string]$parameters.profile_id) -or [string]$parameters.profile_id -match '^REPLACE_WITH_') {
    throw "profile_id must be explicit"
}
$limit = [int64]::MaxValue
foreach ($name in @("acceptance_blocks", "freeze_blocks", "challenge_blocks")) {
    $value = [int64]$parameters.$name
    if ($value -lt 1 -or $value -gt $limit) { throw "$name is outside protocol bounds" }
}
$expectedClose = [System.Numerics.BigInteger]$parameters.acceptance_blocks + [System.Numerics.BigInteger]$parameters.freeze_blocks
if ($expectedClose -gt $limit -or [System.Numerics.BigInteger]$parameters.close_delay_blocks -ne $expectedClose) {
    throw "close_delay_blocks must equal acceptance_blocks + freeze_blocks without overflow"
}
if ([int64]$parameters.funding_confirmation_blocks -lt 1) { throw "funding_confirmation_blocks must be positive" }
if ([int64]$parameters.merchant_delivery_confirmations_required -ne 1) {
    throw "Mainnet canary requires exactly one merchant DeliveryConfirmation"
}
if ([int64]$parameters.custody_attestation_threshold -ne 2 -or
    [int64]$parameters.custody_attestation_participants -ne 3) {
    throw "Mainnet canary custody attestation policy must be 2-of-3"
}
if ([int64]$parameters.max_ledger_entries -ne 64) { throw "max_ledger_entries must be 64" }
if ($parameters.immutable_after_funding -ne $true) { throw "Mainnet parameters must be immutable after Funding creation" }
if ($parameters.challenge_safety_review_status -notin @("PENDING_EXTERNAL_REVIEW", "APPROVED")) {
    throw "Unknown challenge safety review status"
}
if ($parameters.mainnet_approved -ne $false) { throw "Candidate parameters cannot set mainnet_approved=true" }

[pscustomobject]@{
    schema = $parameters.schema
    profile_id = $parameters.profile_id
    close_delay_blocks = [int64]$expectedClose
    merchant_delivery_policy = "1-valid-receipt"
    custody_attestation_policy = "2-of-3"
    challenge_safety_review_status = $parameters.challenge_safety_review_status
    mainnet_approved = $false
    status = "VALID_CANDIDATE_ONLY"
} | ConvertTo-Json -Depth 4
