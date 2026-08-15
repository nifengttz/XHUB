param(
    [string]$ExperimentPath = (Join-Path $PSScriptRoot 'funding-10-mojo.json'),
    [string]$ResultPath = (Join-Path $PSScriptRoot 'funding-10-mojo-result.json'),
    [string]$OutputPath = (Join-Path $PSScriptRoot 'funding-registration.json')
)

$ErrorActionPreference = 'Stop'
$experiment = Get-Content -Raw -LiteralPath $ExperimentPath | ConvertFrom-Json
$result = Get-Content -Raw -LiteralPath $ResultPath | ConvertFrom-Json

if ($experiment.mainnet_approved -ne $false -or $result.mainnet_approved -ne $false) {
    throw 'Mainnet experiment release guard is invalid'
}
if ($experiment.network -ne 'mainnet' -or $result.network -ne 'mainnet') {
    throw 'Network mismatch'
}
if ($result.puzzle_hash -ne $experiment.funding_puzzle_hash) {
    throw 'On-chain Puzzle Hash does not match the experiment'
}
if ([uint64]$result.amount_mojo -ne [uint64]$experiment.funding_amount_mojo) {
    throw 'On-chain amount does not match the experiment'
}
if ($result.spent -ne $false -or $result.verification.success -ne $true) {
    throw 'Funding Coin is not verified and unspent'
}
if (-not $experiment.channel_terms_canonical_hex) {
    throw 'Experiment is missing channel_terms_canonical_hex'
}

$registration = [ordered]@{
    protocol_version = '0x0360'
    funding_coin_id = $result.coin_id
    funding_puzzle_reveal_hex = $experiment.funding_puzzle_reveal
    channel_terms_canonical_hex = $experiment.channel_terms_canonical_hex
}
$registration | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding UTF8
Write-Output $OutputPath
