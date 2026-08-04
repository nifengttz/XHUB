# Stage C Audit Closure Review

Date: 2026-08-03

## Scope

This is an internal independent-style review of the current working tree. The
review covers binary decoding, amount and height bounds, CLVM claim/refund
conditions, signature domains, funding coin lineage, state transitions,
confirmation handling, reorg recovery, restart behavior, and dependency/build
controls.

It is not an external auditor opinion. The current directory has no Git commit,
so the audited object is identified by the file hashes recorded below until a
baseline commit and tag are created.

## Findings

### P1-001: refund height truncation

Status: **CLOSED**

The User watcher previously cast the eight-byte refund height to `u32`, which
could make a large height appear to have already been reached. The decoder now
returns the full `u64` and rejects malformed encodings in
`src/service.rs:537`. Regression coverage is in
`src/hardening_tests.rs:233`.

Evidence:

- `cargo test --locked --all-targets`: 49 passed.
- The regression asserts that `u64::MAX` is not truncated.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.

### P0/P1 review result

No unresolved P0 or P1 funding-safety findings remain from this review. The
claim/refund paths bind funding coin id, parent lineage, output puzzle hashes,
amounts, signature domains, and state transitions. This conclusion is limited
to the reviewed source and simulated tests.

## Evidence matrix

| Control | Evidence | Result |
|---|---|---|
| Binary decoding | Property tests and libFuzzer targets | Harness compiled; long fuzz run pending |
| Amount/height overflow | `MAX_PROTOCOL_U64`, SQLite conversion checks, boundary tests | PASS |
| Signature domain | Rust/CLVM message comparison and mutation tests | PASS |
| CLVM branch/output rules | Simulator claim/refund and malformed-input tests | PASS in simulator |
| Coin lineage | `ASSERT_MY_COIN_ID`, parent/output matching, replay tests | PASS in reviewed code |
| State migration | Randomized transition and restart tests | PASS |
| Crash/restart | PREPARED/PENDING/SUBMITTED recovery tests | PASS |
| Reorg | Settlement rollback tests and duplicate-event tests | PASS in model/simulator |
| Build/lint | fmt, locked test, Clippy, fuzz crate locked check | PASS |
| SBOM | `target/security/sbom.cdx.json` from locked metadata | Generated |
| Dependency advisories | `cargo-audit`, `cargo-deny` | NOT RUN: tools unavailable |
| External audit | Independent third-party review | NOT RUN |
| Immutable audit revision | Git commit/tag | NOT AVAILABLE: working tree has no commit |

## Residual blockers

The formal exit criterion cannot yet be marked closed for three reasons:

1. The repository has no immutable Git revision. Create a baseline commit and
   tag, then regenerate the hash manifest and rerun the evidence commands.
2. `cargo-fuzz` is not installed, so the targets have compiled but have not
   completed the required time-budgeted runs with retained corpus evidence.
3. `cargo-audit` installation timed out in this environment, and `cargo-deny`
   is not installed. The CI workflow is configured to install both and fail on
   advisories or policy violations.

The `HubSigner` trait is a typed application boundary, not proof of process or
host isolation. Production still requires a separate signer trust domain,
authenticated structured IPC, request allowlisting, rate limiting, audit
logging, and key lifecycle controls.

## Reviewed object hashes

SHA-256:

```text
Cargo.toml 5E968EAB86983FB459BE945D87DE9AD86C939BD51ADAB99E59165B97941465C8
Cargo.lock AD3D545DA69D931F44015F3F6F8E8BEFBAE76CD4786620868347DE8529B09085
deny.toml B81A56EAB4AA7BED3EC690B207822F748207B0FEC30073D3C51920F37AC9AA7F
src/lib.rs 8BEAA2104470025B4FACC8FDE89BB3E299D62ABB5EDCC2E5ED81694080BCC85F
src/offchain.rs 17EC169CBA2830A86C6BD5784605F4F0AFFD5A1D716C3ED962B021D9CB8FE177
src/state_store.rs 1E102C21558C32C6EC5E0B5F31892B7465E15952A47B83C516093CEF55CF2EA4
src/service.rs 2F89D0CA13856FB1F30D37AFE8C323DA1264CC76D0680BAE57039D58023F22A7
src/settlement.rs 96D150CD8F4B372A4026A06B404AA94EE79A0A6D4DBAB13580CDC2DF63070DB9
src/chain.rs DAFB784B0EFC9E59F59432BB9E058C3BBA56289E5CB844A38EFA0CA8782B6FA6
src/hardening_tests.rs 641D63C521EACA353E96CC6F3AB0CA171830E03844D4878B806F829432F51EFF
puzzles/wall_hub_channel_v1.clsp B4B4633913D69AA116A93B15AE58540586441318738B01655EEAA0E88E9D1D01
puzzles/wall_hub_channel_v1.clsp.hex 93D349A9FC9BEFE5A933A5EE241E95DC2FB77D9E2DEC6B1376FA8DDBCC548CD5
scripts/security-check.ps1 9B69266897690E6CF961C875062FF4371EE0BE1623F2196E6928E09FC9525098
```
