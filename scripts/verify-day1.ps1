$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Convert-HexToBytes([string] $Hex) {
    if (($Hex.Length % 2) -ne 0 -or $Hex -notmatch '^[0-9a-fA-F]+$') {
        throw "Invalid hex value"
    }

    $bytes = New-Object byte[] ($Hex.Length / 2)
    for ($i = 0; $i -lt $bytes.Length; $i++) {
        $bytes[$i] = [Convert]::ToByte($Hex.Substring($i * 2, 2), 16)
    }
    return $bytes
}

function Convert-BytesToHex([byte[]] $Bytes) {
    return [BitConverter]::ToString($Bytes).Replace('-', '').ToLowerInvariant()
}

function Convert-U16ToBigEndian([uint16] $Value) {
    return [byte[]] @(
        [byte] (($Value -shr 8) -band 0xff),
        [byte] ($Value -band 0xff)
    )
}

function Convert-U64ToBigEndian([uint64] $Value) {
    if ($Value -gt [Int64]::MaxValue) {
        throw "V1 unsigned values must not exceed 2^63 - 1"
    }

    $bytes = [BitConverter]::GetBytes($Value)
    if ([BitConverter]::IsLittleEndian) {
        [Array]::Reverse($bytes)
    }
    return $bytes
}

function Get-Sha256([byte[]] $Bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return $sha.ComputeHash($Bytes)
    }
    finally {
        $sha.Dispose()
    }
}

function Get-FieldBytes($Fields, [string] $Name) {
    $bytes = Convert-HexToBytes $Fields.$Name
    if ($bytes.Length -ne 32) {
        throw "$Name must be exactly 32 bytes"
    }
    return $bytes
}

function Get-ChannelId($Fields) {
    $bytes = [byte[]] (
        [Text.Encoding]::ASCII.GetBytes('WALL_HUB_CHANNEL_V1') +
        (Get-FieldBytes $Fields 'genesis_challenge') +
        (Get-FieldBytes $Fields 'funding_coin_id')
    )
    return Get-Sha256 $bytes
}

function Get-InvoicePreimage($Vector) {
    $c = $Vector.constants
    $f = $Vector.fields
    return [byte[]] (
        [Text.Encoding]::ASCII.GetBytes('WALL_HUB_INVOICE_V1') +
        (Convert-U16ToBigEndian $c.protocol_version) +
        (Get-FieldBytes $f 'genesis_challenge') +
        (Get-FieldBytes $f 'funding_coin_id') +
        (Get-FieldBytes $f 'channel_id') +
        (Get-FieldBytes $f 'order_id') +
        (Get-FieldBytes $f 'merchant_puzzle_hash') +
        (Convert-U64ToBigEndian $c.merchant_amount) +
        (Convert-U64ToBigEndian $f.payment_expiry_height) +
        (Get-FieldBytes $f 'invoice_nonce')
    )
}

function Get-SettlementPreimage($Vector, [string] $Domain = 'WALL_HUB_SETTLEMENT_V1') {
    $c = $Vector.constants
    $f = $Vector.fields
    return [byte[]] (
        [Text.Encoding]::ASCII.GetBytes($Domain) +
        (Convert-U16ToBigEndian $c.protocol_version) +
        (Get-FieldBytes $f 'genesis_challenge') +
        (Get-FieldBytes $f 'funding_coin_id') +
        (Get-FieldBytes $f 'channel_id') +
        (Convert-U64ToBigEndian $c.state_number) +
        (Get-FieldBytes $f 'invoice_hash') +
        (Get-FieldBytes $f 'order_id') +
        (Get-FieldBytes $f 'merchant_puzzle_hash') +
        (Convert-U64ToBigEndian $c.merchant_amount) +
        (Get-FieldBytes $f 'user_puzzle_hash') +
        (Convert-U64ToBigEndian $c.user_remaining_amount) +
        (Get-FieldBytes $f 'nonce') +
        (Convert-U64ToBigEndian $f.payment_expiry_height) +
        (Convert-U64ToBigEndian $f.claim_before_height) +
        (Convert-U64ToBigEndian $f.refund_height) +
        [byte[]] @([byte] $c.fee_policy)
    )
}

function Get-RefundPreimage($Vector) {
    $c = $Vector.constants
    $f = $Vector.fields
    return [byte[]] (
        [Text.Encoding]::ASCII.GetBytes('WALL_HUB_REFUND_V1') +
        (Convert-U16ToBigEndian $c.protocol_version) +
        (Get-FieldBytes $f 'genesis_challenge') +
        (Get-FieldBytes $f 'funding_coin_id') +
        (Get-FieldBytes $f 'channel_id') +
        (Get-FieldBytes $f 'user_puzzle_hash') +
        (Convert-U64ToBigEndian $c.funding_amount) +
        (Convert-U64ToBigEndian $f.refund_height) +
        [byte[]] @([byte] $c.fee_policy)
    )
}

function Copy-Vector($Vector) {
    return ($Vector | ConvertTo-Json -Depth 10 | ConvertFrom-Json)
}

function Assert-Equal($Actual, $Expected, [string] $Label) {
    if ($Actual -ne $Expected) {
        throw "$Label mismatch. Expected $Expected, got $Actual"
    }
    Write-Host "PASS  $Label"
}

function Assert-True([bool] $Condition, [string] $Label) {
    if (-not $Condition) {
        throw "$Label failed"
    }
    Write-Host "PASS  $Label"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$vectorPath = Join-Path $repoRoot 'test-vectors\day1-v1.json'
$vector = Get-Content -Raw -LiteralPath $vectorPath | ConvertFrom-Json

$channelId = Convert-BytesToHex (Get-ChannelId $vector.fields)
Assert-Equal $channelId $vector.fields.channel_id 'channel id derivation'

$invoicePreimage = Get-InvoicePreimage $vector
$invoiceHash = Convert-BytesToHex (Get-Sha256 $invoicePreimage)
Assert-Equal $invoicePreimage.Length $vector.expected.invoice_preimage_length 'invoice preimage length'
Assert-Equal $invoiceHash $vector.expected.invoice_hash 'invoice hash vector'
Assert-Equal $invoiceHash $vector.fields.invoice_hash 'settlement invoice binding'

$settlementPreimage = Get-SettlementPreimage $vector
$settlementHashBytes = Get-Sha256 $settlementPreimage
$settlementHash = Convert-BytesToHex $settlementHashBytes
Assert-Equal $settlementPreimage.Length $vector.expected.settlement_preimage_length 'settlement preimage length'
Assert-Equal $settlementHash $vector.expected.settlement_hash 'settlement hash vector'

$claimMessage = [byte[]] (
    $settlementHashBytes +
    (Get-FieldBytes $vector.fields 'funding_coin_id') +
    (Get-FieldBytes $vector.fields 'agg_sig_me_additional_data')
)
Assert-Equal (Convert-BytesToHex $claimMessage) $vector.expected.claim_signature_message 'AGG_SIG_ME claim message'

$refundPreimage = Get-RefundPreimage $vector
$refundHashBytes = Get-Sha256 $refundPreimage
$refundHash = Convert-BytesToHex $refundHashBytes
Assert-Equal $refundPreimage.Length $vector.expected.refund_preimage_length 'refund preimage length'
Assert-Equal $refundHash $vector.expected.refund_hash 'refund hash vector'

$refundMessage = [byte[]] (
    $refundHashBytes +
    (Get-FieldBytes $vector.fields 'funding_coin_id') +
    (Get-FieldBytes $vector.fields 'agg_sig_me_additional_data')
)
Assert-Equal (Convert-BytesToHex $refundMessage) $vector.expected.refund_signature_message 'AGG_SIG_ME refund message'

Assert-True ($vector.constants.funding_amount -eq ($vector.constants.merchant_amount + $vector.constants.user_remaining_amount)) 'amount conservation'
Assert-True ($vector.constants.state_number -eq 1) 'one-shot state number'
Assert-True ($vector.constants.fee_policy -eq 0) 'external-only fee policy'
Assert-True (($vector.fields.claim_before_height + 1) -eq $vector.fields.refund_height) 'non-overlapping claim/refund boundary'
Assert-True (($vector.fields.payment_expiry_height + $vector.constants.min_claim_window_blocks) -le $vector.fields.claim_before_height) 'minimum claim window'

$boundaryHeight = [uint64] $vector.fields.claim_before_height
foreach ($height in @(($boundaryHeight - 1), $boundaryHeight, ($boundaryHeight + 1))) {
    $claimValid = $height -le $vector.fields.claim_before_height
    $refundValid = $height -ge $vector.fields.refund_height
    Assert-True (-not ($claimValid -and $refundValid)) "branch exclusivity at height $height"
}

$mutations = @(
    @{ name = 'protocol_domain'; mutate = { param($v) } },
    @{ name = 'protocol_version'; mutate = { param($v) $v.constants.protocol_version = 2 } },
    @{ name = 'genesis_challenge'; mutate = { param($v) $v.fields.genesis_challenge = '01' + $v.fields.genesis_challenge.Substring(2) } },
    @{ name = 'funding_coin_id'; mutate = { param($v) $v.fields.funding_coin_id = '02' + $v.fields.funding_coin_id.Substring(2) } },
    @{ name = 'channel_id'; mutate = { param($v) $v.fields.channel_id = '03' + $v.fields.channel_id.Substring(2) } },
    @{ name = 'state_number'; mutate = { param($v) $v.constants.state_number = 2 } },
    @{ name = 'invoice_hash'; mutate = { param($v) $v.fields.invoice_hash = '08' + $v.fields.invoice_hash.Substring(2) } },
    @{ name = 'order_id'; mutate = { param($v) $v.fields.order_id = '04' + $v.fields.order_id.Substring(2) } },
    @{ name = 'merchant_puzzle_hash'; mutate = { param($v) $v.fields.merchant_puzzle_hash = '05' + $v.fields.merchant_puzzle_hash.Substring(2) } },
    @{ name = 'merchant_amount'; mutate = { param($v) $v.constants.merchant_amount++ } },
    @{ name = 'user_puzzle_hash'; mutate = { param($v) $v.fields.user_puzzle_hash = '06' + $v.fields.user_puzzle_hash.Substring(2) } },
    @{ name = 'user_remaining_amount'; mutate = { param($v) $v.constants.user_remaining_amount++ } },
    @{ name = 'nonce'; mutate = { param($v) $v.fields.nonce = '07' + $v.fields.nonce.Substring(2) } },
    @{ name = 'payment_expiry_height'; mutate = { param($v) $v.fields.payment_expiry_height++ } },
    @{ name = 'claim_before_height'; mutate = { param($v) $v.fields.claim_before_height++ } },
    @{ name = 'refund_height'; mutate = { param($v) $v.fields.refund_height++ } },
    @{ name = 'fee_policy'; mutate = { param($v) $v.constants.fee_policy = 1 } }
)

foreach ($mutation in $mutations) {
    $copy = Copy-Vector $vector
    & $mutation.mutate $copy
    $domain = if ($mutation.name -eq 'protocol_domain') { 'XALL_HUB_SETTLEMENT_V1' } else { 'WALL_HUB_SETTLEMENT_V1' }
    $mutatedHash = Convert-BytesToHex (Get-Sha256 (Get-SettlementPreimage $copy $domain))
    Assert-True ($mutatedHash -ne $settlementHash) "signed-field mutation: $($mutation.name)"
}

Write-Host ''
Write-Host 'DAY 1 ACCEPTANCE: PASS'
