# XHUB V3.6 HUB single-VPS Docker test

This profile runs one V3.6 HUB beside the three single-VPS Watchtower containers. It binds only to `127.0.0.1:18737`, uses the trusted public HTTPS Coinset endpoint, and routes RecoveryPackage delivery to `wt-a`, `wt-b`, and `wt-c`.

The deployment is test-only. It does not expose a public port, generate a SpendBundle, broadcast a transaction, or satisfy production failure-domain requirements.

Build from the `V3.6` directory:

```bash
docker build -f deploy/mainnet/docker-single-vps-hub/Dockerfile -t xhub-hub-v3-6:test .
```

The server keeps HUB data and secrets under `/opt/xhub-v3.6-hub-test`. The three existing Watchtower API Token files are mounted read-only from `/opt/xhub-v3.6-test/secrets`; their contents are not copied into JSON or Compose.

The quorum delivery endpoint is:

```text
POST /api/v3.6/funding-coins/{coin_id}/recovery-packages/{sequence}/watchtower-quorum-deliveries
```

It returns success after at least two of the three Watchtowers durably accept the same RecoveryPackage. Failed retryable recipients can be retried with the same idempotency key.

Register a confirmed Funding Coin from the server so the HUB API token never leaves the VPS:

```bash
./register-funding-coin.sh /path/to/funding-candidate.json <funding-coin-id>
```

The script validates the candidate as V3.6 mainnet input, constructs the versioned request, and requires HTTP `201` from the loopback-only HUB endpoint.

Rotate the local HUB API token after suspected disclosure:

```bash
./rotate-hub-api-token.sh
```

This replaces only the HUB API token, restarts the HUB container, and waits for its Docker health check to pass.

Submit an independently verified reservation and deliver its RecoveryPackage to all three local Watchtowers:

```bash
./run-three-watchtower-canary.sh /path/to/verified-hub-reservation-request.json
```

The script requires a persisted `SIGNED` reservation, a matching HUB RecoveryPackage, a successful fixed `2-of-3` delivery quorum, and byte-identical persisted packages on all three Watchtowers. It stores non-secret audit artifacts under `/home/ubuntu/xhub-v36-canary-artifacts` by default.

Install the public merchant confirmer identity used by the 5-mojo canary:

```bash
./install-canary-confirmer.sh /path/to/confirmers.single-vps.local.json
```

The installer validates the exact public identity, backs up the previous public config, restarts the three Watchtower containers, and waits for all health checks. It never reads or uploads the merchant private key.

Complete the single-VPS greenlight after generating a merchant DeliveryConfirmation locally:

```bash
./complete-single-vps-greenlight.sh /path/to/delivery-confirmation.json <funding-coin-id> 1
```

The script submits the merchant confirmation, compares all three unsigned custody payloads, signs with each locally mounted test attester key in a network-disabled one-shot container, cross-submits the attestations, and requires `test_ready=true` while preserving `production_ready=false`.
