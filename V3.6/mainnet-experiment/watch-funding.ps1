param(
    [string]$ExperimentPath = (Join-Path $PSScriptRoot 'funding-10-mojo.json'),
    [switch]$Wait,
    [int]$PollSeconds = 15
)

$ErrorActionPreference = 'Stop'
$experiment = Get-Content -Raw -LiteralPath $ExperimentPath | ConvertFrom-Json
if ($experiment.schema -ne 'xhub-v3-6-mainnet-experiment-1') {
    throw 'Unsupported experiment schema'
}
if ($experiment.network -ne 'mainnet' -or $experiment.mainnet_approved -ne $false) {
    throw 'Experiment network or release guard is invalid'
}

$body = @{
    puzzle_hash = "0x$($experiment.funding_puzzle_hash)"
    include_spent_coins = $true
} | ConvertTo-Json -Compress

function Convert-HexToBytes([string]$Hex) {
    $normalized = $Hex -replace '^0x', ''
    if (($normalized.Length % 2) -ne 0) { throw 'Invalid hex length' }
    $bytes = New-Object byte[] ($normalized.Length / 2)
    for ($index = 0; $index -lt $bytes.Length; $index++) {
        $bytes[$index] = [Convert]::ToByte($normalized.Substring($index * 2, 2), 16)
    }
    return $bytes
}

function Get-CoinId($Coin) {
    $parent = Convert-HexToBytes $Coin.parent_coin_info
    $puzzleHash = Convert-HexToBytes $Coin.puzzle_hash
    $amount = [uint64]$Coin.amount
    $amountBytes = [BitConverter]::GetBytes($amount)
    if ([BitConverter]::IsLittleEndian) { [Array]::Reverse($amountBytes) }

    $first = 0
    while ($first -lt 8 -and $amountBytes[$first] -eq 0) { $first++ }
    if ($first -lt 8 -and (($amountBytes[$first] -band 0x80) -ne 0)) { $first-- }
    $encodedAmountLength = 8 - $first
    $preimage = New-Object byte[] (64 + $encodedAmountLength)
    $parent.CopyTo($preimage, 0)
    $puzzleHash.CopyTo($preimage, 32)
    if ($encodedAmountLength -gt 0) {
        $amountBytes[$first..7].CopyTo($preimage, 64)
    }
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return -join ($sha256.ComputeHash($preimage) | ForEach-Object { $_.ToString('x2') })
    } finally {
        $sha256.Dispose()
    }
}

do {
    $response = Invoke-RestMethod `
        -Method Post `
        -Uri "$($experiment.rpc_url)/get_coin_records_by_puzzle_hash" `
        -ContentType 'application/json' `
        -Body $body
    if (-not $response.success) {
        throw "Coinset rejected the query: $($response.error)"
    }
    $matches = @($response.coin_records | Where-Object {
        [uint64]$_.coin.amount -eq [uint64]$experiment.funding_amount_mojo
    })
    if ($matches.Count -gt 0) {
        $matches | ForEach-Object {
            [pscustomobject]@{
                coin_id = if ($_.name) { $_.name -replace '^0x', '' } else { Get-CoinId $_.coin }
                parent_coin_info = $_.coin.parent_coin_info
                puzzle_hash = $_.coin.puzzle_hash
                amount_mojo = [uint64]$_.coin.amount
                confirmed_block_index = [uint32]$_.confirmed_block_index
                spent = [bool]$_.spent
                spent_block_index = [uint32]$_.spent_block_index
                timestamp = [uint64]$_.timestamp
            }
        } | ConvertTo-Json -Depth 4
        exit 0
    }
    if (-not $Wait) {
        [pscustomobject]@{
            status = 'NOT_FOUND'
            puzzle_hash = $experiment.funding_puzzle_hash
            amount_mojo = [uint64]$experiment.funding_amount_mojo
        } | ConvertTo-Json
        exit 0
    }
    Start-Sleep -Seconds ([Math]::Max(5, $PollSeconds))
} while ($true)
