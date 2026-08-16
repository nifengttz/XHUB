# V3.6 local payment signer

`xhub-local-signer-v3-6` is the local, non-custodial signing component for the
V3.6 5-mojo recovery canary. It has no HTTP client, RPC client, SpendBundle
builder, or broadcast capability.

It accepts only an exact V3.6 mainnet request for a 5-mojo channel with a
1-mojo off-chain reservation. Before signing it verifies the canonical channel
terms, Funding Coin binding, user public key, user remainder target, request
and reservation nonces, and authorization hash.

The user BLS secret must remain in a local 32-byte hexadecimal file controlled
by the user. Do not place that file in this repository, upload it, or pass its
contents on the command line.

Review a request without reading a secret:

```powershell
cargo run --locked --manifest-path .\Cargo.toml --bin xhub-local-signer-v3-6 -- `
  inspect .\payment-request.json
```

After verifying the review output, sign locally. The confirmation argument is
deliberately required and the output path must not already exist:

```powershell
cargo run --locked --manifest-path .\Cargo.toml --bin xhub-local-signer-v3-6 -- `
  sign .\payment-request.json D:\secure\xhub-user-bls.hex .\payment-request.signed.json `
  --confirm-offchain-1-mojo
```

The result is a signed request JSON that can be independently inspected again.
Signing is strictly off-chain: the tool reports `spend_bundle_created=false`,
`push_tx_called=false`, and `chain_broadcast=false`.

## Windows graphical launcher

For a click-through workflow, place `Start-XHUB-V3.6-Signer.cmd`,
`Start-XHUB-V3.6-Signer.ps1`, and `xhub-local-signer-v3-6.exe` in the same
local folder. Double-click the `.cmd` file. The launcher first shows the full
request review, then asks for a locally stored user BLS secret file only after
the operator confirms the exact 1-mojo off-chain authorization. It does not
upload the secret or retain it after the process exits.
