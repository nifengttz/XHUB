# HUB HTTP API v1

This is the Stage B network boundary for the current one-shot channel MVP. It
is intended for local or controlled mainnet experiments. It is not yet a
production Internet service.

## Start

```bash
cargo build --release --bin hub-api
./target/release/hub-api --config hub-api.json
```

`hub-api.json` fields:

| Field | Meaning |
| --- | --- |
| `listen_addr` | TCP bind address, for example `127.0.0.1:8080` or `0.0.0.0:8080` |
| `db_path` | SQLite state database |
| `rpc_url` | Mainnet Coinset RPC, normally `https://api.coinset.org` |
| `hub_secret_key` | 32-byte BLS secret key encoded as 64 hex characters |
| `api_key` | Required value of the `X-API-Key` request header |
| `agg_sig_me_additional_data` | Optional 32-byte hex value; defaults to mainnet |

Never commit `hub-api.json`, the Hub secret key, or the API key.

## Health

```text
GET /healthz
```

This endpoint does not require authentication and returns the Hub public key,
network name, and mainnet Genesis Challenge.

## Merchant browser workbench

Open `http://<hub-host>:8080/merchant` from the merchant computer. The page
checks the HUB, collects the Invoice and Voucher fields, and calls the same
authenticated API endpoints. The merchant must enter the configured `api_key`
in the page; it is retained only in the browser session for this controlled
test. The user private key is never entered into the page: the CHIA client still
produces the signed `PaymentIntent` that is pasted into the Voucher form.

## Invoice signing

```text
POST /v1/invoices
X-API-Key: <api-key>
Content-Type: application/json
```

Request:

```json
{
  "request_id": "merchant-req-001",
  "idempotency_key": "invoice:order-001",
  "channel": {
    "user_public_key": "<96 hex characters>",
    "hub_public_key": "<96 hex characters>",
    "user_puzzle_hash": "<64 hex characters>",
    "genesis_challenge": "<64 hex characters>",
    "claim_before_height": 130,
    "refund_height": 131
  },
  "funding_coin_id": "<64 hex characters>",
  "order_id": "<64 hex characters>",
  "merchant_puzzle_hash": "<64 hex characters>",
  "payment_expiry_height": 105,
  "invoice_nonce": "<64 hex characters>"
}
```

The response contains `invoice_hex`, the fixed binary Invoice to pass to the
User and Merchant. The endpoint creates the channel's local `FUNDED` record;
the caller must only use this after the funding coin is actually confirmed.

## Voucher signing

```text
POST /v1/vouchers
X-API-Key: <api-key>
Content-Type: application/json
```

Request fields are the same `channel`, plus:

```json
{
  "request_id": "merchant-req-002",
  "idempotency_key": "voucher:order-001",
  "channel": { "...": "same channel fields as the invoice request" },
  "invoice_hex": "<invoice_hex returned by /v1/invoices>",
  "intent_hex": "<User-signed PaymentIntent hex>"
}
```

The endpoint verifies the Invoice and User signature, rejects expired or
wrong-channel data, then atomically persists and returns `voucher_hex`. A
repeat request with the same Intent returns the persisted Voucher; a different
Intent for an already issued channel is rejected.

The service reads the current mainnet peak height from `rpc_url` before every
Invoice or Voucher signature. The request must not provide a height. Mainnet
Genesis Challenge and `AGG_SIG_ME` additional data are taken from the server's
mainnet configuration.

## Error handling and limits

All errors are JSON objects with `error.code` and `error.message`. Requests
larger than 64 KiB are rejected. The server binds plain HTTP and has no TLS or
user account system; put it behind a private network or reverse proxy for any
non-local test. The public Coinset endpoint is acceptable for a controlled
test, but a self-operated Full Node RPC is recommended for production.
