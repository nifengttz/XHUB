# Wall-Hub MVP

This repository contains the implementation artifacts for the seven-day
Wall-Hub MVP.

Day 1 freezes the one-shot payment-channel protocol. Day 2 implements its
funding puzzle and verifies the claim/refund paths in Chia's simulator. Day 3
implements the off-chain Invoice, Intent, and Voucher signing lifecycle. Day 4
adds a transactional SQLite state machine and restart-safe artifact storage.
Day 5 connects persisted Vouchers to simulator Claim/Refund submission,
independent fee coins, and confirmation-gated terminal states.
Day 6 adds the attack, replay, restart, and Claim/Refund boundary-race matrix,
including an explicit `CLAIM_EXPIRED` merchant status.
Day 7 packages both settlement outcomes into a clean, reproducible simulator
demo and produces the final MVP verdict.

## Stage A mainnet adapter

The Stage A foundation is now available in `src/chain.rs`. The acceptance
baseline is Chia mainnet with full node 2.7.3, the mainnet Genesis Challenge,
three-block confirmation depth, and an independent fee coin. The adapter also
supports the public testnet11 Coinset RPC for development probes.
The adapter provides:

- network/genesis/sync validation and peak tracking;
- coin records, funding-coin children, broadcast, and mempool queries;
- confirmation-depth polling with transport retry;
- mempool-floor fee estimation and independent fee-coin selection;
- fee-coin change outputs;
- SQLite chain observations, final children, and reorg rollback evidence.

Probe the public testnet endpoint with:

```powershell
cargo run --example testnet_probe
```

For a local full node, construct `ChiaRpcConfig::FullNode` with the full node
RPC URL and the certificate/key files under the node's `config/ssl` tree.
The repository does not contain wallet keys or mainnet funding, so the 20-run
Claim/Refund acceptance campaign must be run only after those external
prerequisites are supplied. The evidence schema and required fields are
listed in `docs/mainnet-acceptance-template.md`. Mainnet reorg acceptance is
observation-based; the project does not intentionally induce a mainnet reorg.

## One-command final demo

Run from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\demo-day7.ps1
```

The script compiles CLVM, verifies protocol vectors, demonstrates offline
Merchant Claim and no-Voucher User Refund, runs all attack/regression tests,
and applies strict linting. A successful run ends with
`WALL-HUB MVP FINAL RESULT: PASS`.

## Day 1 verification

Run from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
```

The command verifies the normative hashes, field-mutation coverage, amount
conservation, and the claim/refund height boundary.

## Day 2 build and verification

Prerequisites are Rust stable and `clvm_tools_rs 0.4.0`:

```powershell
cargo install clvm_tools_rs --version 0.4.0 --locked
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The simulator suite proves the Merchant can construct and submit the claim
after the User and Hub signing keys are released. It also covers the refund
branch, fixed outputs, exact funding amount, signature binding, replay, and the
height boundary.

## Day 3 off-chain lifecycle

The Rust library provides:

- `InvoiceFields` and `MerchantInvoice` for Hub-authorized orders;
- `SettlementCommitment` with the exact canonical CLVM message;
- `PaymentIntent` for the User signature;
- `PaymentVoucher` for Hub verification, co-signing, and signature aggregation;
- `MerchantPaymentStatus` for `Pending`, `PendingHub`, `PaidOffchain`, and
  `Expired` / `ClaimExpired` display states;
- typed `ProtocolError` results for wrong network, coin, key, fields, signature,
  expiry, or claim window.

`cargo test --all-targets` covers all 16 Settlement fields and all 9 Invoice
fields individually, then submits the resulting valid aggregate signature to
the CLVM simulator.

## Day 4 state and persistence

`ChannelStore` persists channel state, order id, nonce, Intent, Voucher, and
settlement balances in SQLite. Composite primary keys reject duplicate orders
and nonces per channel. An immediate transaction acquires channel ownership
before the Hub signs a concurrent Intent, so losing requests never receive a
second Voucher.

The persisted lifecycle is:

```text
FUNDED -> INTENT_SIGNED -> VOUCHER_ISSUED
       -> CLAIM_SUBMITTED -> SETTLED

FUNDED -> REFUNDABLE -> REFUND_SUBMITTED -> REFUNDED
```

Tests close and reopen the SQLite connection at every state and compare the
recovered Intent and Voucher byte-for-byte.

## Day 5 settlement integration

`build_claim_bundle` lets a Merchant settle from the funding coin, public
channel arguments, and persisted Voucher without access to User or Hub keys.
`build_refund_bundle` returns the full funding amount to the User after the
refund height. An optional independently funded standard coin can pay fees
without changing either channel output.

Submission records only `CLAIM_SUBMITTED` or `REFUND_SUBMITTED`. The store
enters `SETTLED` or `REFUNDED` only after confirmed funding-coin children match
the expected parent, puzzle hashes, amounts, and output count exactly.

## Day 6 attack and recovery matrix

The suite mutates every signed Invoice and Settlement field, rejects duplicate
and cross-channel/network redemption, restores a persisted Voucher after a Hub
restart, and races Claim against Refund at both boundary heights. An issued
Voucher remains `PAID_OFFCHAIN` through the inclusive Claim cutoff and becomes
`CLAIM_EXPIRED` when the Refund branch opens.

## Protocol documents

- `docs/protocol-v1.md`: normative protocol and binary encoding
- `docs/state-machine-v1.md`: lifecycle and error semantics
- `docs/day1-acceptance.md`: Day 1 acceptance record
- `docs/day2-acceptance.md`: Day 2 simulator acceptance record
- `docs/day3-acceptance.md`: Day 3 off-chain lifecycle acceptance record
- `docs/day4-acceptance.md`: Day 4 state and persistence acceptance record
- `docs/day5-acceptance.md`: Day 5 settlement integration acceptance record
- `docs/day6-acceptance.md`: Day 6 attack and recovery acceptance record
- `docs/day7-final-report.md`: Day 7 reproducible demo and final verdict
- `docs/WALL_HUB_7_DAY_MVP_SUMMARY_ZH.md`: Chinese seven-day proof summary and next-stage roadmap
- `test-vectors/day1-v1.json`: deterministic interoperability vectors
- `puzzles/wall_hub_channel_v1.clsp`: funding puzzle source
- `src/lib.rs`: Rust encoding, SpendBundle construction, and simulator tests
- `src/offchain.rs`: Invoice, Intent, Voucher, validation, and status logic
- `src/state_store.rs`: SQLite schema, transactions, state transitions, and recovery
- `src/settlement.rs`: Claim/Refund/fee bundles and confirmation tracking
- `src/day6_tests.rs`: replay, restart, boundary-race, and status tests
- `examples/day7_demo.rs`: clean Claim and Refund simulator demonstration
- `scripts/demo-day7.ps1`: one-command final verification

## Stage B service CLI and watchers

Stage B now includes three independent binaries. They exchange only versioned
JSON envelopes; the payload remains the fixed binary Invoice, Intent, or
Voucher encoding used by the protocol:

```powershell
cargo run --bin user -- --help
cargo run --bin hub -- --help
cargo run --bin merchant -- --help
```

Encode an artifact for a role boundary with an idempotency key:

```powershell
cargo run --bin merchant -- artifact encode Voucher <payload_hex> <channel_id> claim:<channel_id>
```

The `user` and `merchant` binaries also support a persistent watcher:

```powershell
cargo run --bin merchant -- watch .\merchant-watch.json
cargo run --bin user -- watch .\user-watch.json
cargo run --bin merchant -- metrics .\merchant.sqlite3
```

Use `--once` for failure-injection tests. The watcher config contains the
SQLite path, channel parameters, RPC URL, confirmation depth, and polling
interval. Only the User config contains `user_secret_key`; the Merchant
watcher constructs Claim from the persisted Voucher and never needs User or
Hub private keys.

Broadcast preparation, serialized SpendBundle, transaction id, attempt state,
mempool observations, confirmation observations, and audit events are stored
in SQLite. Re-running the same idempotency key reuses the same SpendBundle;
restarting a watcher resumes recoverable broadcast jobs.

`metrics <db_path>` returns counts for channels, broadcast jobs, recoverable
jobs, attempts, confirmed jobs, and reorg observations. Audit records are
available through `ChannelStore::list_audit_events` for operator tooling.

## Stage C engineering hardening

The current hardening baseline is documented in `docs/stage-c-hardening.md`.
The internal closure review and remaining release blockers are in
`docs/stage-c-audit-closure.md`.
Run the local gate with:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\security-check.ps1
```

The gate runs formatting, locked tests, Clippy, and deterministic CycloneDX
SBOM generation. Install `cargo-audit` and `cargo-deny` before treating the
dependency checks as release evidence. The libFuzzer targets are under `fuzz/`
and are run with `scripts/run-fuzz.ps1`.
