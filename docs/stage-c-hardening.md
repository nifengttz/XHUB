# Stage C Engineering Hardening

Date: 2026-08-03

This document records the engineering controls implemented before an
independent security audit. It is not an audit opinion and does not authorize
mainnet operation by itself.

## Implemented

- Property tests exercise arbitrary protocol bytes, canonical round trips,
  mutated artifacts, height boundaries, and randomized state transitions.
- Arbitrary CLVM tree encodings are parsed under panic-catching tests and two
  libFuzzer targets cover protocol bytes and CLVM solution encodings.
- Restart tests cover the durable broadcast states `PREPARED`, `PENDING`, and
  `SUBMITTED`; duplicate state operations and read-only database failures are
  verified to leave state consistent.
- Broadcast idempotency keys are bound to channel, operation kind, and funding
  coin. Attempt counters reject overflow instead of saturating.
- Protocol heights and persisted unsigned values are bounded by `i64::MAX`,
  matching SQLite's signed integer representation.
- Hub signing is exposed to the state store through the typed `HubSigner`
  capability. It only supports Invoice and Claim signing operations; the
  database layer no longer requires a raw `SecretKey` type.
- `Cargo.lock`, `deny.toml`, a deterministic SBOM generator, local security
  checks, and CI checks are present.

## Required release evidence

Run `cargo fuzz` for both targets with a fixed time budget and preserve the
corpus and crash disposition. Install and run `cargo-audit` and `cargo-deny`.
Record Rust/toolchain versions, the source revision, SBOM hash, test output,
and any dependency advisories.

The `HubSigner` trait is an application boundary, not proof of process or host
isolation. Production deployment still needs a separate signer process or
host, authenticated structured IPC, request allowlisting, rate limiting,
audit logging, and private-key lifecycle controls.

Independent review remains required for CLVM cost, signature domains, coin
lineage, time semantics, and conservation invariants. P0/P1 closure requires
an issue, fix revision, regression test, and reviewer or auditor re-test.
