# Wall-Hub Parameterized Channel Protocol V2

Status: **DRAFT - NOT FOR MAINNET FUNDS**

V2 is a new protocol. It does not change, reinterpret, or share a puzzle hash
with frozen V1. It defines a one-funding-coin channel with a cooperative claim
branch and a user-only refund branch.

## 1. Parameters

All integers are unsigned, fixed-width big-endian values. `amount_mojos` is a
`u64` in the range `1..=2^63-1`; decimal XCH and floating point are forbidden.

| Curried field | Size |
|---|---:|
| `protocol_version` (`0x0002`) | 2 |
| `genesis_challenge` | 32 |
| `agg_sig_me_additional_data` | 32 |
| `hub_public_key`, `user_public_key` | 48 each |
| `user_puzzle_hash` | 32 |
| `funding_amount` | 8 |
| `claim_before_height`, `refund_height` | 8 each |

`refund_height` MUST be greater than `claim_before_height`. The Hub calculates
both absolute heights from a fresh, synced peak; its product policy MUST reject
a requested refund delay below the published minimum. A wallet MUST enforce its
own stricter minimum if present. This relative-delay policy cannot be enforced
by a puzzle that sees only absolute heights.

The funding output MUST have the exact curried V2 puzzle hash and exact
`funding_amount`. Fees MUST come from an independent input coin.

## 2. Identifiers and hashes

`SHA256` consumes the literal concatenation of fields below. Hex strings,
JSON, CLVM serialization, addresses, and length prefixes are never inputs.

```text
channel_id = SHA256("WALL_HUB_CHANNEL_V2" || genesis_challenge || funding_coin_id)

channel_terms_hash = SHA256(
  "WALL_HUB_TERMS_V2" || 0x0002 || genesis_challenge ||
  agg_sig_me_additional_data || hub_public_key || user_public_key ||
  user_puzzle_hash || funding_amount || claim_before_height || refund_height
)

settlement_hash = SHA256(
  "WALL_HUB_SETTLEMENT_V2" || 0x0002 || genesis_challenge || funding_coin_id ||
  channel_id || channel_terms_hash || merchant_puzzle_hash || settlement_amount ||
  user_puzzle_hash || user_remainder || settlement_nonce || claim_before_height ||
  refund_height || 0x00
)

refund_hash = SHA256(
  "WALL_HUB_REFUND_V2" || 0x0002 || genesis_challenge || funding_coin_id ||
  channel_id || channel_terms_hash || user_puzzle_hash || funding_amount ||
  refund_height || 0x00
)
```

Every hash field is 32 bytes; public keys are 48 bytes; heights and amounts
are 8 bytes. `funding_coin_id` is the Chia coin ID of the exact V2 funding
output. `settlement_amount + user_remainder == funding_amount` and both
amounts are positive, so a claim cannot create dust-free zero outputs or lose
funds.

## 3. Puzzle behavior

The source at `puzzles/wall_hub_channel_v2.clsp` is normative once compiled
and its module tree hash is published in the vector file. It accepts only:

```text
CLAIM: ASSERT_MY_COIN_ID, ASSERT_MY_AMOUNT, ASSERT_BEFORE_HEIGHT_ABSOLUTE,
       AGG_SIG_ME(user_public_key, settlement_hash),
       AGG_SIG_ME(hub_public_key, settlement_hash),
       CREATE_COIN(merchant_puzzle_hash, settlement_amount),
       CREATE_COIN(user_puzzle_hash, user_remainder)

REFUND: ASSERT_MY_COIN_ID, ASSERT_MY_AMOUNT, ASSERT_HEIGHT_ABSOLUTE,
        AGG_SIG_ME(user_public_key, refund_hash),
        CREATE_COIN(user_puzzle_hash, funding_amount)
```

The branches consume the same coin and are height-disjoint. A claim requires
both BLS signatures; a refund requires only the user signature after
`refund_height`. The solution supplies no delegated conditions.

## 4. Signing

For Chia `AGG_SIG_ME`, the raw message passed to BLS is respectively:

```text
settlement_hash || funding_coin_id || agg_sig_me_additional_data
refund_hash     || funding_coin_id || agg_sig_me_additional_data
```

The Hub MUST sign a settlement only after independently observing an unspent,
confirmed funding coin. Neither party signs arbitrary messages for a web page.

## 5. Required validation

Wallets and Hub MUST reject wrong field sizes, unknown protocol versions,
zero/overflow amounts, network mismatch, an expired request, a terms hash or
puzzle hash they cannot recompute, and a funding output with an inexact amount.
They MUST bind `request_id`, `session_id`, ephemeral session public keys,
network, terms hash, funding puzzle hash, and both heights in the connect
signature defined by `xhub-connect-v1.md`.

Before status `ACTIVE`, Hub RPC must confirm the output is unspent, has the
expected puzzle hash and amount, and has `required_confirmations`. Reorgs move
the channel out of `ACTIVE` until this check succeeds again.

## 6. Test and audit gate

V1 regression tests MUST remain unchanged. V2 requires simulator tests for
claim/refund boundaries, amount conservation, altered curried fields, altered
solution fields, cross-coin replay, wrong network, duplicate request, and
reorg recovery. This draft is not a production authorization; publish the
compiled module hash, independent test results, and a CLVM security audit
before mainnet use.
