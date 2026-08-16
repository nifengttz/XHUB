# XHUB Wallet V3.6 core

This is a strict single-address Chia mainnet wallet for the V3.6 canary.

- It generates and restores 24-word English BIP-39 mnemonics.
- It derives only `m/12381/8444/2/0` and exposes no address-index control.
- It derives the Chia standard puzzle hash and `xch` Bech32m address.
- The Windows UI has no local password and does not use Windows account protection.
- The Windows UI stores wallet material in `wallet-v3_6.plaintext.json` beside the launcher and automatically opens it on startup.
- Mnemonic, master private key, index-0 wallet private key, and synthetic private key can be displayed in plaintext.
- The transaction preview command validates fields but cannot create a SpendBundle, call RPC, invoke `push_tx`, sign, or broadcast.

## Security boundary

The passwordless design is intentionally unprotected at rest. Anyone who can copy
`wallet-v3_6.plaintext.json` can control the wallet. Use it only for a deliberately
small mainnet canary amount whose complete loss is acceptable.

An actual mainnet broadcast remains outside this wallet-core milestone and needs a
transaction-specific approval naming the Funding Coin, amount, purpose, maximum
fee, and permission to broadcast.
