# Stage A Mainnet Acceptance

This is the active Stage A acceptance template. The test baseline is Chia
mainnet with full node `2.7.3`, the mainnet Genesis Challenge, three-block
confirmation depth, and an independent fee coin. A zero-fee transaction may
be recorded as a compatibility sample, but it does not satisfy the fee-policy
acceptance item.

## Fixed environment

| Field | Value |
|---|---|
| Network | `mainnet` |
| Full node version | `2.7.3` |
| RPC endpoint | `https://127.0.0.1:8555` |
| Genesis Challenge | `ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb` |
| Confirmation depth | `3` blocks |
| Fee policy | independent fee coin; dynamic estimate plus configured margin |
| Reorg policy | observe natural mainnet reorgs; do not induce one |
| Observation database | |

## Required evidence per run

Record the node version, peak height/hash, funding coin id, transaction id,
mempool result, fee coin id, fee amount, inclusion height, three-confirmation
height, retry count, final children, and any reorg observation. Do not record
seed phrases or private keys.

## Claim runs

| Run | Funding coin id | Fee coin id | Transaction id | Mempool accepted | Inclusion height | 3-confirm height | Fee | Children | Reorg result | Result |
|---:|---|---|---|---|---:|---:|---:|---|---|---|
| 1 | | | | | | | | | | |
| 2 | | | | | | | | | | |
| ... | | | | | | | | | | |
| 20 | | | | | | | | | | |

## Refund runs

| Run | Funding coin id | Fee coin id | Transaction id | Mempool accepted | Inclusion height | 3-confirm height | Fee | Children | Reorg result | Result |
|---:|---|---|---|---|---:|---:|---:|---|---|---|
| 1 | | | | | | | | | | |
| 2 | | | | | | | | | | |
| ... | | | | | | | | | | |
| 20 | | | | | | | | | | |

## Boundary checks

- Claim at `claim_before_height`: PASS / FAIL
- Claim at `refund_height`: PASS / FAIL
- Refund at `claim_before_height`: PASS / FAIL
- Refund at `refund_height`: PASS / FAIL
- Only the expected funding coin children exist: PASS / FAIL
- Funding coin output amount remains exact: PASS / FAIL
- Independent fee coin and change output are correct: PASS / FAIL
- Natural mainnet reorg observed and state rolled back correctly: PASS / FAIL / NOT OBSERVED

## Exit decision

- Claim: 20 consecutive successes after 3 confirmations: PASS / FAIL
- Refund: 20 consecutive successes after 3 confirmations: PASS / FAIL
- Independent fee coin campaign: PASS / FAIL
- Natural mainnet reorg evidence: PASS / NOT OBSERVED
- Stage A exit: PASS / FAIL / PENDING NATURAL REORG EVIDENCE
