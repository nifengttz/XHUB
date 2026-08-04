# Stage A Testnet Acceptance

This report is intentionally evidence-first. A row is `PASS` only when the
transaction id, observed peak, confirmation height, fee, funding coin, and
final children are present in the SQLite chain-observation log.

## Environment

| Field | Value |
|---|---|
| Network | `testnet11` |
| Full node version | |
| RPC endpoint | |
| Genesis challenge | |
| Initial peak height/hash | |
| Confirmation depth | |
| Fee margin | |
| Observation database | |

## Claim runs

| Run | Funding coin id | Transaction id | Mempool accepted | Confirmed height | Peak height | Fee | Children | Reorg result | Result |
|---:|---|---|---|---:|---:|---:|---|---|---|
| 1 | | | | | | | | | |
| 2 | | | | | | | | | |
| ... | | | | | | | | | | |
| 20 | | | | | | | | | | |

## Refund runs

| Run | Funding coin id | Transaction id | Mempool accepted | Confirmed height | Peak height | Fee | Children | Reorg result | Result |
|---:|---|---|---|---:|---:|---:|---|---|---|
| 1 | | | | | | | | | |
| 2 | | | | | | | | | | |
| ... | | | | | | | | | | |
| 20 | | | | | | | | | | |

## Boundary and rollback drills

- Claim at `claim_before_height`: PASS / FAIL
- Claim at `refund_height`: PASS / FAIL
- Refund at `claim_before_height`: PASS / FAIL
- Refund at `refund_height`: PASS / FAIL
- Confirmed Claim output removed by short reorg and state returned to
  `CLAIM_SUBMITTED`: PASS / FAIL
- Confirmed Refund output removed by short reorg and state returned to
  `REFUND_SUBMITTED`: PASS / FAIL

## Exit decision

- Claim: 20 consecutive successes: PASS / FAIL
- Refund: 20 consecutive successes: PASS / FAIL
- At least one reorg/confirmation rollback drill: PASS / FAIL
- Stage A exit: PASS / FAIL
