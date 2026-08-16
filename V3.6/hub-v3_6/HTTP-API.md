# XHUB HTTP API V3.6

Status: `VECTOR_READY`. This document fixes the test-vector API surface. It is not a production deployment approval.

## Version and encoding

- Every request uses the `/api/v3.6` prefix and the header `x-xhub-protocol-version: 0x0360`.
- POST bodies and GET query strings also carry `protocol_version=0x0360`. A missing or different value is rejected before processing.
- Requests accept lower- or uppercase hex with an optional `0x` prefix. Responses always emit lowercase hex without a prefix.
- Amounts are canonical unsigned decimal strings. Leading zeroes, zero, and values above `2^63-1` are rejected.
- JSON is transport only. Consensus hashes and signatures use the canonical binary encoding from `protocol-v3_6`.
- `signed_result_canonical_hex` and `recovery_package_canonical_hex` expose those canonical bytes for independent verification.

## Endpoints

```text
POST /api/v3.6/funding-coins
POST /api/v3.6/reservations
GET  /api/v3.6/funding-coins/{funding_coin_id}/reservations/{reservation_nonce}
GET  /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/latest
GET  /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}
POST /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}/deliveries
GET  /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}/deliveries
```

Funding registration accepts `funding_coin_id`, `funding_puzzle_reveal_hex`,
and `channel_terms_canonical_hex`. The HUB recomputes the puzzle hash from the
reveal, queries the configured Chia mainnet source, verifies the amount and
unspent state, and applies the configured confirmation threshold before
activating the channel.

The reservation POST accepts `request_id`, `funding_coin_id`, the four `LedgerEntry` fields, and `user_authorization_signature`. The server obtains both chain snapshots through `ChainStateProvider`; no client height is accepted.

The reservation lookup key is exactly `(funding_coin_id, reservation_nonce)`. A lost POST response is recovered by querying with that original nonce. A client must not create a replacement nonce for the same payment while the result is unknown.

Delivery POST bodies contain:

```json
{
  "protocol_version": "0x0360",
  "recipient_id": "watchtower-1",
  "recipient_kind": "WATCHTOWER",
  "idempotency_key": "delivery-1"
}
```

`recipient_kind` is `MERCHANT` or `WATCHTOWER`. The persisted delivery binding includes the Funding Coin, state sequence, checkpoint hash, RecoveryPackage content hash, recipient, and idempotency key. The same completed key returns the existing record. A retryable failure increments `attempt_count` and reuses the same key; conflicting content is never overwritten.

## Client actions

| Status or class | `result_class` | `client_action` | `ledger_written` | Required behavior |
|---|---|---|---|---|
| `SIGNED`, `DELIVERED` | `SUCCESS` | `ACCEPT` | `true` | Verify the signed result and accept it. |
| Deterministic rejection | `REJECTED` | `STOP` | `false` | Verify the signed result; do not retry payment. |
| `PENDING`, `UNKNOWN` | `UNKNOWN` | `RETRY_SAME_NONCE` | `null` | Query with the original nonce. |
| `RPC_UNAVAILABLE`, `INTERNAL_ERROR` | `UNKNOWN` | `RETRY_SAME_NONCE` | `null` | Never infer that the ledger was not written; query with the original nonce. |
| `NODE_NOT_SYNCED`, `CHAIN_STATE_UNCERTAIN`, `CHANNEL_REORG_PENDING` | `UNKNOWN` | `PAUSE_AND_QUERY` | `null` | Pause new reservations and query the original nonce until chain state is certain. |
| `NONCE_CONFLICT` | `REJECTED` | `STOP` | `false` | Do not issue a second payment or overwrite the first result. |

An unsigned HTTP validation failure is also `REJECTED/STOP/false`, but it is not a protocol `SignedReservationResult`. Deterministic business decisions made after a valid request are returned through the persisted signed result.

## Delivery state machine

```text
new or retryable request -> PENDING -> DELIVERED
                                  \-> FAILED_RETRYABLE -> PENDING on same key
                                  \-> FAILED_FINAL
```

The transport adapter receives the same idempotency key and the exact persisted RecoveryPackage. A `DELIVERED` or `FAILED_FINAL` record is terminal. A process crash may leave `PENDING`; retrying the same key resumes delivery without changing package content.
