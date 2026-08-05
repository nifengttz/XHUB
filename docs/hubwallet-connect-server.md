# HubWallet Connect Server Setup

Status: **development integration boundary**

The wallet-connect endpoints are deliberately separate from the V1 `X-API-Key`
merchant API. They use a temporary, signed capability URI and do not accept an
API key from a browser.

## Configuration

Add these fields to the existing `hub-api.json`:

```json
{
  "connect_public_base_url": "https://api.xhub.example",
  "connect_hub_key_id": "xhub-main-2026-01",
  "connect_request_secret_key": "<32-byte BLS secret key hex>",
  "connect_noise_private_key": "<32-byte X25519 private key hex>"
}
```

`connect_request_secret_key` MUST be a distinct, rotatable identity key. It
must not be the V1 settlement key and must not be committed or bundled into the
web application. The public half is exposed at
`/.well-known/xhub-connect-keys.json`; wallets must consume it only through a
pre-trusted, signed registry update, never merely because a page supplied it.

`connect_noise_private_key` is the Hub's Noise XX static key. A missing key
disables Relay processing; the Hub does not fall back to a generated production
identity.

## Current endpoints

`POST /v1/wallet-connect/requests` accepts XCH mojo amount, refund delay, and
network. It validates mainnet XCH, creates a SQLite-backed request valid for at
most five minutes, signs the exact canonical URI preimage, and returns a
`hubwallet://connect` URI.

`GET /v1/wallet-connect/requests/{request_id}` returns display-safe request
state only. It never returns keys, puzzle hashes, a spend bundle, or wallet
metadata.

`GET /.well-known/xhub-connect-keys.json` returns the active public request
signing key. It is a distribution endpoint, not a trust decision.

`POST /v1/wallet-connect/relay/{request_id}/handshake` processes base64url
Noise XX handshake frames. `POST /v1/wallet-connect/relay/{request_id}/messages`
accepts only established encrypted frames. The first valid `wallet_hello`
atomically pairs its request, derives V2 terms from a fresh mainnet peak, and
returns an encrypted, BLS-signed `funding_request_final`. Serve both endpoints
only behind TLS and a rate-limiting reverse proxy.

## Still required before public use

- TLS termination, strict Origin/CORS policy, rate limiting, and authenticated
  operational access.
- Persisted V2 funding terms; independent RPC confirmation, reorg handling,
  and activation only after exact unspent coin validation.
- Signed multi-key registry with not-before/not-after and revocation metadata.
