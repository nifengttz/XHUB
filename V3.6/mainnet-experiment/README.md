# V3.6 Mainnet 10 Mojo Experiment

This directory is an unaudited mainnet experiment, not a mainnet release.
`mainnet_approved` remains `false`.

The immutable transaction target is recorded in `funding-10-mojo.json`:

- network: Chia mainnet
- RPC: `https://api.coinset.org`
- funding amount: `10 mojo`
- fee: `0 mojo`
- wallet fingerprint: `1648103239`
- broadcast: requires explicit confirmation in HUBWALLET

The Hub BLS secret is stored under `../local-secrets` and is excluded from Git.
Do not move it into this directory or commit it.

Before broadcast, verify Coinset has no matching historical Coin:

```powershell
./watch-funding.ps1
```

After HUBWALLET broadcasts, wait for the real Coin ID:

```powershell
./watch-funding.ps1 -Wait
```

Coinset may omit the derived `name` field from puzzle-hash queries. The script
then derives the Coin ID with Chia's canonical amount encoding and SHA-256,
and the result must still be verified with `get_coin_record_by_name` before
HUB registration.

After the by-name verification is recorded, build the versioned HUB request:

```powershell
./build-registration.ps1
```

The HUB must independently read the same Coin from Coinset and wait for the
frozen test activation depth of 32 confirmations before marking it `ACTIVE`.

The `1 mojo` merchant-payment experiment is off-chain and never broadcasts a
Funding Coin spend. First authorize `payment-1-mojo.json` in HUBWALLET. The
wallet must show the exact Funding Coin, `1 mojo`, and a `9 mojo` remainder,
and it must only sign after explicit confirmation.

After the signed full request has replaced `payment-1-mojo.json`, run:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./run-payment.ps1
```

The script independently verifies the wallet BLS signature, submits the
reservation with the original nonce, delivers the RecoveryPackage to the
configured watchtower, records the merchant DeliveryConfirmation, and requires
a threshold-1 greenlight. Public evidence is written to
`payment-1-mojo-result.json`; transient request files remain under `private`.

## Closing simulation

`closing-simulation-result.json` is the public report for the state-sequence-1
recovery Closing simulation. The private RecoveryPackage remains under
`private/` and is excluded from Git.

The simulation verifies the RecoveryPackage bindings and signatures, then runs
the Funding, Initial Closing `FINALIZE`, and Merchant Payment CLVM locally. It
checks the frozen relative and absolute height assertions, the Funding Coin ID
and amount, the derived output Coin IDs, and the exact `1 mojo` merchant / `9
mojo` user split.

This is a local CLVM simulation only. It does not create a SpendBundle, is not
broadcast-ready, and does not broadcast or spend the real Funding Coin. The
hypothetical earliest Start Close height is `9150074`; any future broadcast
must wait for chain eligibility and requires a new explicit user confirmation.

To reproduce the report from the private package:

```powershell
cargo run --offline --manifest-path ../wallet-v3_6/Cargo.toml `
  --bin mainnet-closing -- `
  ./private/closing-recovery-package.json 9150074 `
  ./closing-simulation-result.json
```

## Watchtower monitor

`watchtower-monitor-result.json` records a real read-only Coinset poll by the
independent Watchtower monitor. The persisted RecoveryPackage binds the RPC
CoinRecord to the Funding puzzle reveal before the monitor makes a decision.

At the recorded peak the Funding Coin remained unspent, so the result is
`FUNDING_OPEN`. No challenge plan or SpendBundle was created, and no broadcast
was attempted. When Closing begins, the monitor derives the expected Initial
and Subsequent Closing Coin IDs from confirmed CoinSpend solutions and locally
runs the CHALLENGE CLVM only for a strictly newer complete RecoveryPackage.
