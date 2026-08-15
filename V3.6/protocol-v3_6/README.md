# XHUB Protocol V3.6

Independent Rust implementation of V3.6 protocol types, canonical encodings, hashes, BLS signatures, Merkle rules, and golden vectors.

## Documents

- [Protocol specification](protocol-v3_6.md)
- [Implementation specification](IMPLEMENTATION-SPEC.md)
- [Freeze checklist](FREEZE-CHECKLIST.md)

## Verify

```powershell
cargo test --manifest-path .\Cargo.toml
cargo run --manifest-path .\Cargo.toml --bin generate-vectors
```

The generated vector file is `test-vectors/protocol-v3_6.json`. Test seeds in that file are public fixtures and must never hold funds.
