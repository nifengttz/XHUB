# Mainnet 10 Mojo Test Report

Date: 2026-08-03
Network: Chia mainnet
Wallet fingerprint: 3895043750
Wallet ID: 1

## Scope

This report records one real mainnet Claim and one real mainnet Refund using
10 mojo funding coins. The funding amount was not used to pay transaction fees.
Both test bundles were broadcast with a zero fee and accepted by the local
full node.

## Pre-test Wallet State

- Wallet height: 9097387
- Sync status: Synced
- Spendable balance: 11240399880 mojo
- Full node: `https://127.0.0.1:8555`
- Genesis challenge matched mainnet: `ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb`

## Claim Test

Funding channel puzzle hash:
`03a15cff7496e045aab67b8ad3d9e866232bc4b8b84e04f52621fa2afeb744eb`

- Funding coin: `d69a24145fe33299f42e119362007629224419e272d71878fcfe96ff6ed7c8b5`
- Funding amount: 10 mojo
- Funding confirmation height: 9097414
- Claim cutoff: 9097451
- Refund height: 9097452
- Transaction id: `e32541cdb188d9300db8c5c0756afd30d883103c0407e9fdcad118ce36a535a6`
- Confirmation height: 9097428
- Channel id: `238f62d4f43e907f2415ead8e73768d01bf5a1668a2940e91e928640341af4cb`
- Fee: 0 mojo
- Mempool cost: 26812617
- Mempool conditions: removal 10 mojo, additions 1 mojo + 9 mojo, no CLVM error

Final children:

| Child coin | Amount | Confirmation height |
|---|---:|---:|
| `aa9f6bb455d32bc581967ee0d5afa33240f05875fb514fabcaf3d941b7d86c69` | 1 mojo | 9097428 |
| `260c31602285b22e0f43211c2df91410981352a3a964090be1c31fa06242afd5` | 9 mojo | 9097428 |

Result: PASS. The funding coin produced exactly 1 mojo + 9 mojo and was spent
once.

## Refund Test

Funding channel puzzle hash:
`531e23ae642982b37f08c2a91a4d61012b41981ab500a83272a4c6ad2da53ac3`

- Funding coin: `1e5a96a3dbdbf86aad80371b84ca9a345b6a74f59e9cbb45c642f2d1639edfb7`
- Funding amount: 10 mojo
- Funding confirmation height: 9097431
- Refund activation height: 9097413
- Transaction id: `3f741a941809b4ea44b18cc47baa13ae6b05c671900c7f3203b6790011a9e9c1`
- Confirmation height: 9097434
- Fee: 0 mojo
- Mempool conditions: removal 10 mojo, addition 10 mojo to the User puzzle hash

Final child:

| Child coin | Amount | Confirmation height |
|---|---:|---:|
| `682fad3c0fa83735f37c754bd1630e07c6dea8c3fcd1f21e68a6ca0c2da72f99` | 10 mojo | 9097434 |

Result: PASS. The refund branch returned the complete 10 mojo funding amount.

## Verification

- Rust tests: 38 passed, 0 failed
- Clippy with `-D warnings`: PASS
- Day 1 protocol vector verification: PASS
- Real mainnet Claim: PASS
- Real mainnet Refund: PASS
- Funding amount conservation: PASS
- Reorg rollback test: NOT RUN
- 20 consecutive Claim runs: NOT RUN
- 20 consecutive Refund runs: NOT RUN
- Independent fee coin selection on mainnet: NOT RUN; both bundles used zero fee

## Notes

The first confirmation polling attempt exposed a Chia 2.7.3 RPC response
compatibility issue: some coin-record responses use `null` for
`spent_block_index` when spent records are excluded, while the pinned SDK type
expects an integer. Final confirmation and children were therefore checked
with the native Chia RPC response. The adapter request path was adjusted to
request complete coin-record shapes and filter spent records locally; the
broad wallet-address query still needs a follow-up against this node's exact
response shape.

No seed phrase or private key was read or included in this report.
