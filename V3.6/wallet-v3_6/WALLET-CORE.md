# XHUB Wallet V3.6 core

This is a strict single-address Chia mainnet wallet for the V3.6 canary.

- It generates and restores 24-word English BIP-39 mnemonics.
- It derives only `m/12381/8444/2/0` and exposes no address-index control.
- It derives the Chia standard puzzle hash and `xch` Bech32m address.
- The Windows UI has no local password and does not use Windows account protection.
- The Windows UI stores wallet material in `wallet-v3_6.plaintext.json` beside the launcher and automatically opens it on startup.
- Mnemonic, master private key, index-0 wallet private key, and synthetic private key can be displayed in plaintext.
- The chain sync command validates mainnet and reads all CoinRecords for the index-0 puzzle hash to calculate confirmed balance and Coin history.
- Sending is split into two explicit phases. `prepare-send` reads current unspent CoinRecords, selects at most 100 inputs, builds a standard Chia spend, signs with the index-0 synthetic key, and validates the SpendBundle with mainnet consensus rules without broadcasting.
- `broadcast` accepts only a previously prepared transaction, verifies the SpendBundle ID and consensus conditions again, rechecks every selected Coin as unspent, and then calls `push_tx`.
- The Windows UI displays destination, amount, fee, change, selected Coin IDs, purpose, RPC URL, and SpendBundle ID in a transaction-specific confirmation before invoking `broadcast`.

## Security boundary

The passwordless design is intentionally unprotected at rest. Anyone who can copy
`wallet-v3_6.plaintext.json` can control the wallet. Use it only for a deliberately
small mainnet canary amount whose complete loss is acceptable.

An actual mainnet broadcast is performed only after transaction-specific approval
naming the selected input Coins, amount, purpose, fee, destination, change, RPC,
and SpendBundle ID. The Windows wallet's final confirmation dialog is that
approval; closing it or choosing No performs no broadcast.

The default chain source is `https://api.coinset.org`. It is a third-party public
RPC endpoint. The wallet requires HTTPS, rejects embedded URL credentials, and
verifies the endpoint reports Chia mainnet. Balance and history are coin-level,
matching Chia's additions/removals model rather than an account transaction log.
