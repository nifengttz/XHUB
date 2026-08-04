# Wall-Hub One-Shot Payment Channel Protocol V1

Status: **FROZEN FOR MVP IMPLEMENTATION**

Date: 2026-08-03

Normative words `MUST`, `MUST NOT`, `SHOULD`, and `MAY` are used as protocol
requirements.

## 1. Security claim

V1 proves one narrow claim:

> After User and Hub sign one settlement for one confirmed funding coin, any
> holder of the resulting Voucher can submit the claim spend no later than the
> simulator cutoff height `C`. The spend pays exactly 1 mojo to Merchant and 9 mojos to User,
> without further cooperation from User or Hub.

V1 is a one-shot, time-bounded payment channel. It is not a general multi-state
channel and does not contain an old-state challenge protocol.

## 2. Constants

| Name | Value | Encoding |
|---|---:|---|
| `PROTOCOL_VERSION` | 1 | unsigned 16-bit big-endian |
| `FUNDING_AMOUNT` | 10 mojos | unsigned 64-bit big-endian |
| `MERCHANT_AMOUNT` | 1 mojo | unsigned 64-bit big-endian |
| `USER_REMAINDER` | 9 mojos | unsigned 64-bit big-endian |
| `STATE_NUMBER` | 1 | unsigned 64-bit big-endian |
| `FEE_POLICY` | 0 (`EXTERNAL_FEE_ONLY`) | unsigned byte |
| `MIN_CLAIM_WINDOW_BLOCKS` | 20 | protocol validation rule |

All unsigned 64-bit values MUST be at most `2^63 - 1`. The CLVM puzzle MUST
normalize fixed-width numeric bytes through CLVM arithmetic before emitting a
numeric consensus condition.

The funding amount is not allowed to pay transaction fees. If a fee is needed,
an independent fee coin MAY be aggregated into the SpendBundle.

## 3. Identities and funding descriptor

The funding puzzle is constructed with these immutable parameters:

| Field | Size | Requirement |
|---|---:|---|
| `protocol_version` | 2 bytes | MUST equal `0x0001` |
| `genesis_challenge` | 32 bytes | target network identifier |
| `agg_sig_me_additional_data` | 32 bytes | target network consensus constant |
| `user_public_key` | 48 bytes | BLS public key used by claim and refund branches |
| `hub_public_key` | 48 bytes | BLS public key used by claim branch |
| `user_puzzle_hash` | 32 bytes | fixed destination for change and refund |
| `claim_before_height` | 8 bytes | unsigned big-endian simulator cutoff `C` |
| `refund_height` | 8 bytes | unsigned big-endian activation height `R = C + 1` |

After the funding coin is confirmed:

```text
funding_coin_id = ChiaCoinId(parent_coin_id, funding_puzzle_hash, 10 mojos)

channel_id = SHA256(
    ASCII("WALL_HUB_CHANNEL_V1") ||
    genesis_challenge ||
    funding_coin_id
)
```

The puzzle cannot curry its own coin id without a circular dependency. The
claim/refund solution therefore supplies `funding_coin_id`, and the puzzle MUST
emit `ASSERT_MY_COIN_ID` for that exact value.

## 4. Protocol objects

### 4.1 MerchantInvoice

`MerchantInvoice` is created from a merchant order and authorized by Hub:

```text
invoice_fields
invoice_hash
hub_invoice_signature
```

The Hub invoice signature uses `AugSchemeMPL` over the 32-byte `invoice_hash`.
It authorizes presentation of the order but cannot spend the funding coin.

### 4.2 PaymentIntent

```text
settlement_fields
settlement_hash
user_claim_signature
```

`user_claim_signature` is an `AGG_SIG_ME`-compatible signature over the claim
message defined in section 7.

### 4.3 PaymentVoucher

```text
settlement_fields
settlement_hash
user_claim_signature
hub_claim_signature
```

Both signatures MUST cover the same claim message. A Merchant MUST NOT report
`PAID_OFFCHAIN` until both individual signatures verify against the public keys
committed by the funding puzzle.

### 4.4 SettlementCommitment

`SettlementCommitment` is the fixed 329-byte preimage defined in section 6.2.
It includes `invoice_hash`, so an Intent and Voucher are bound to the exact
Hub-authorized Invoice rather than only to a reused business order id.

### 4.5 MerchantClaim

`MerchantClaim` is a funding-coin spend using the Voucher claim branch. No
merchant signature is required. Anyone may relay the spend because its outputs
are fixed by the signed settlement.

## 5. Timing rules

For V1, the claim cutoff and refund activation MUST be consecutive heights:

```text
claim_before_height == C
refund_height == R == C + 1
payment_expiry_height + MIN_CLAIM_WINDOW_BLOCKS <= C
```

- In the target `chia-sdk-test 0.33.0` simulator, a claim is accepted through
  height `C` and rejected at `R`.
- A refund is rejected through height `C` and accepted at or after `R`.
- Broadcasting by `C` is not sufficient; simulator acceptance must occur by `C`.
- User and Hub MUST NOT sign after `payment_expiry_height`.
- Merchant MUST verify the current height before accepting a Voucher.
- If Merchant does not obtain acceptance by `C`, the Voucher loses its
  on-chain execution right. This is an explicit V1 limitation.

Compatibility note: Chia consensus checks `ASSERT_BEFORE_HEIGHT_ABSOLUTE` against
the previous transaction-block height with strict-before semantics, while the
target simulator accepts equality. The one-block separation preserves branch
exclusivity in the target simulator and may create a conservative one-block gap
under the stricter check; deployment outside this simulator requires a fresh
boundary test.

## 6. Canonical encodings

All hashes use SHA-256 over the exact concatenation below. ASCII domains are
literal bytes without a terminator. Every other byte field has the exact size
listed. Implementations MUST reject fields with the wrong size and MUST NOT
serialize JSON, CLVM lists, display addresses, decimal XCH values, or hex text
into these preimages.

### 6.1 Invoice hash

```text
invoice_hash = SHA256(invoice_preimage)
```

| Offset | Length | Field |
|---:|---:|---|
| 0 | 19 | ASCII `WALL_HUB_INVOICE_V1` |
| 19 | 2 | `protocol_version` |
| 21 | 32 | `genesis_challenge` |
| 53 | 32 | `funding_coin_id` |
| 85 | 32 | `channel_id` |
| 117 | 32 | `order_id` |
| 149 | 32 | `merchant_puzzle_hash` |
| 181 | 8 | `merchant_amount` |
| 189 | 8 | `payment_expiry_height` |
| 197 | 32 | `invoice_nonce` |

Total length: 229 bytes.

### 6.2 Settlement hash

```text
settlement_hash = SHA256(settlement_preimage)
```

| Offset | Length | Field |
|---:|---:|---|
| 0 | 22 | ASCII `WALL_HUB_SETTLEMENT_V1` |
| 22 | 2 | `protocol_version` |
| 24 | 32 | `genesis_challenge` |
| 56 | 32 | `funding_coin_id` |
| 88 | 32 | `channel_id` |
| 120 | 8 | `state_number` |
| 128 | 32 | `invoice_hash` |
| 160 | 32 | `order_id` |
| 192 | 32 | `merchant_puzzle_hash` |
| 224 | 8 | `merchant_amount` |
| 232 | 32 | `user_puzzle_hash` |
| 264 | 8 | `user_remaining_amount` |
| 272 | 32 | `nonce` |
| 304 | 8 | `payment_expiry_height` |
| 312 | 8 | `claim_before_height` |
| 320 | 8 | `refund_height` |
| 328 | 1 | `fee_policy` |

Total length: 329 bytes.

### 6.3 Refund hash

```text
refund_hash = SHA256(refund_preimage)
```

| Offset | Length | Field |
|---:|---:|---|
| 0 | 18 | ASCII `WALL_HUB_REFUND_V1` |
| 18 | 2 | `protocol_version` |
| 20 | 32 | `genesis_challenge` |
| 52 | 32 | `funding_coin_id` |
| 84 | 32 | `channel_id` |
| 116 | 32 | `user_puzzle_hash` |
| 148 | 8 | `funding_amount` |
| 156 | 8 | `refund_height` |
| 164 | 1 | `fee_policy` |

Total length: 165 bytes.

## 7. Signature messages

The Hub invoice signature signs `invoice_hash` with `AugSchemeMPL`.

User and Hub claim signatures MUST be directly usable by the funding puzzle's
two `AGG_SIG_ME` conditions. The bytes passed to `AugSchemeMPL.sign` are:

```text
claim_signature_message =
    settlement_hash || funding_coin_id || agg_sig_me_additional_data
```

The User refund signature uses:

```text
refund_signature_message =
    refund_hash || funding_coin_id || agg_sig_me_additional_data
```

Implementations MUST obtain `agg_sig_me_additional_data` from the target
network's consensus constants. They MUST NOT assume it for an arbitrary custom
network.

## 8. Funding coin spend graph

The top-level solution contains a branch tag. The puzzle MUST reject any tag
other than `CLAIM` or `REFUND` and MUST NOT execute delegated conditions.

### 8.1 CLAIM branch

The puzzle validates all field sizes and constants, recomputes `channel_id` and
`settlement_hash`, and emits only these effective conditions:

```text
ASSERT_MY_COIN_ID funding_coin_id
ASSERT_MY_AMOUNT 10
ASSERT_BEFORE_HEIGHT_ABSOLUTE C
AGG_SIG_ME user_public_key settlement_hash
AGG_SIG_ME hub_public_key settlement_hash
CREATE_COIN merchant_puzzle_hash 1
CREATE_COIN user_puzzle_hash 9
```

It MUST additionally enforce:

```text
state_number == 1
merchant_amount == 1
user_remaining_amount == 9
merchant_amount + user_remaining_amount == funding_amount
user_puzzle_hash == curried user_puzzle_hash
refund_height == claim_before_height + 1
fee_policy == 0
payment_expiry_height + 20 <= C
```

### 8.2 REFUND branch

The puzzle recomputes `channel_id` and `refund_hash`, then emits only:

```text
ASSERT_MY_COIN_ID funding_coin_id
ASSERT_MY_AMOUNT 10
ASSERT_HEIGHT_ABSOLUTE R
AGG_SIG_ME user_public_key refund_hash
CREATE_COIN user_puzzle_hash 10
```

## 9. Replay and finality properties

- Same-coin replay is prevented because the funding coin can be spent once.
- Cross-coin replay is prevented by `AGG_SIG_ME` and `funding_coin_id`.
- Cross-network replay is prevented by the consensus additional data and the
  signed `genesis_challenge`.
- Cross-channel replay is prevented by `funding_coin_id` and `channel_id`.
- Field tampering changes the signed settlement hash.
- Claim and refund cannot both succeed because their height intervals do not
  overlap and both consume the same funding coin.
- `order_id` and `nonce` uniqueness before signing are state-machine rules; the
  single funding coin provides the final on-chain one-spend boundary.

## 10. Frozen decisions and deferred work

| Decision | V1 result |
|---|---|
| Old-state challenge | Not applicable: only `state_number = 1` is accepted |
| Merchant offline forever | Not supported; Merchant must settle by `C` |
| Hub refuses to sign | No Voucher exists; Merchant must not mark paid |
| User recovery without Voucher | User refund at or after `R` |
| Fees | External fee coin only |
| Multiple merchants/payments | Deferred |
| Watcher | Manual Merchant monitoring is sufficient for MVP only |

No P0 protocol decision is left open after the Day 2 CLVM implementation.
