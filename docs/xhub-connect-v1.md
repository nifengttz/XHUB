# X-Hub Connect v1 Canonical Encoding and Signatures

Status: **DRAFT - paired implementation required**

This document defines bytes shared by X-Hub and HubWallet. JSON is a transport
representation only and MUST NOT be signed.

## Canonical binary primitives

`u16`, `u32`, and `u64` are unsigned big-endian. A byte string is encoded as
`u32(length) || bytes`; ASCII literals have no terminator. UUIDs are 16 raw
RFC 4122 bytes. Fixed fields have no length prefix. Optional values are
`u8(0)` or `u8(1) || value`. A decoder rejects trailing bytes, duplicate JSON
keys, unknown critical fields, non-minimal integers, and invalid UTF-8.

## URI signature

The URI signature is an AugSchemeMPL signature by the Hub request key:

```text
SHA256("XHUB_CONNECT_URI_V1" || u16(1) || request_uri || request_id ||
       u64(expires_at) || hub_key_id)
```

Here `request_uri` and `hub_key_id` are UTF-8 byte strings encoded with the
primitive above. `request_id` is raw UUID bytes. The URI query MUST contain
exactly `v`, `request_uri`, `request_id`, `expires_at`, `hub_key_id`, and
`sig`; parameters are percent-decoded once, then checked before verification.
The wallet resolves `hub_key_id` only through its trusted key registry.

## Final funding request signature

The encrypted payload uses the following field order, then SHA-256 and
AugSchemeMPL signing:

```text
"XHUB_FUNDING_REQUEST_V1" || u16(1) || request_id || session_id ||
u64(created_at) || u64(expires_at) || network || origin || hub_key_id ||
asset_id || amount_mojos || max_fee_mojos || required_confirmations ||
channel_protocol_version || hub_public_key || user_public_key ||
user_puzzle_hash || claim_before_height || refund_height ||
funding_puzzle_hash || channel_terms_hash || wallet_session_public_key || nonce
```

Text fields use the byte-string primitive. `network` is its canonical ASCII
identifier (for example `mainnet`); `asset_id` is `xch`. Amounts/heights are
`u64`; confirmations are `u32`; hashes are 32 bytes; BLS keys are 48 bytes;
the X25519 wallet session public key and nonce are 32 bytes. The signature key
is identified by `hub_key_id`, which is included in the signed preimage.

## Session transport

Use Noise XX over X25519 with ChaCha20-Poly1305 and SHA-256. The Relay sees
only `{session_id, sequence, ciphertext}`. Each direction has a monotonically
increasing `u64` sequence number; repeats, gaps, a changed session ID, and
messages after expiry are rejected. The wallet sends `wallet_hello` only after
URI verification; Hub atomically consumes the request when accepting the first
valid hello and returns the same final request for an idempotent retry.

Hub keys are distinct from channel settlement keys. The trusted registry
contains `hub_key_id`, public key, allowed origins, not-before, not-after, and
revocation state. A registry update needs an independently trusted software
update or root-signed metadata channel.
