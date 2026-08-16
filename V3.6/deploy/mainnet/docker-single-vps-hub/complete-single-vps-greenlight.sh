#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <delivery-confirmation.json> <funding-coin-id> <state-sequence>" >&2
  exit 2
fi

confirmation_file=$1
funding_coin_id=${2#0x}
state_sequence=$3
entry_index=0
artifact_dir=${XHUB_CANARY_ARTIFACT_DIR:-/home/ubuntu/xhub-v36-canary-artifacts}/greenlight-state-$state_sequence
signer_image=${XHUB_CUSTODY_SIGNER_IMAGE:-xhub-custody-attest-v3-6:test}
mkdir -p "$artifact_dir"

for tower in a b c; do
  case "$tower" in
    a) port=18738 ;;
    b) port=18739 ;;
    c) port=18740 ;;
  esac
  tower_token=$(sudo cat "/opt/xhub-v3.6-test/secrets/wt-$tower-api-token.txt")
  confirmation_response="$artifact_dir/wt-$tower-confirmation-response.json"
  confirmation_http=$(curl --silent --show-error --max-time 30 \
    --output "$confirmation_response" --write-out '%{http_code}' \
    --request POST "http://127.0.0.1:$port/api/v3.6/delivery-confirmations" \
    --header "Authorization: Bearer $tower_token" \
    --header 'x-xhub-protocol-version: 0x0360' \
    --header 'content-type: application/json' \
    --data-binary "@$confirmation_file")
  if [[ "$confirmation_http" != "200" ]]; then
    cat "$confirmation_response"
    printf '\nwatchtower=%s confirmation_http=%s\n' "$tower" "$confirmation_http" >&2
    exit 1
  fi

  payload_file="$artifact_dir/wt-$tower-custody-payload.json"
  payload_http=$(curl --silent --show-error --max-time 30 \
    --output "$payload_file" --write-out '%{http_code}' \
    "http://127.0.0.1:$port/api/v3.6/funding-coins/$funding_coin_id/states/$state_sequence/entries/$entry_index/custody-attestation?protocol_version=0x0360" \
    --header "Authorization: Bearer $tower_token" \
    --header 'x-xhub-protocol-version: 0x0360')
  unset tower_token
  if [[ "$payload_http" != "200" ]]; then
    cat "$payload_file"
    printf '\nwatchtower=%s payload_http=%s\n' "$tower" "$payload_http" >&2
    exit 1
  fi
done

python3 - "$artifact_dir" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
payloads = [json.loads((root / f"wt-{tower}-custody-payload.json").read_text()) for tower in "abc"]
binding = {
    (payload["custody_attestation_hash"], payload["custody_attestation_canonical_hex"])
    for payload in payloads
}
if len(binding) != 1:
    raise SystemExit("Watchtower custody signing payloads differ")
print("custody_payloads_match=true")
PY

for attester in a b c; do
  signed_file="$artifact_dir/wt-$attester-signed-attestation.json"
  : >"$signed_file"
  chmod 666 "$signed_file"
  sudo docker run --rm --user 0:0 \
    --userns host \
    --network none \
    --read-only \
    --cap-drop ALL \
    --security-opt no-new-privileges:true \
    --pids-limit 64 \
    --volume "$artifact_dir/wt-a-custody-payload.json:/input/payload.json:ro" \
    --volume "/opt/xhub-v3.6-test/operator-secrets/wt-$attester-bls-secret-key.hex:/input/secret.hex:ro" \
    --volume "$signed_file:/output.json" \
    "$signer_image" \
    /input/payload.json "wt-$attester" single-tencent-vps /input/secret.hex /output.json
  chmod 600 "$signed_file"
done

for tower in a b c; do
  case "$tower" in
    a) port=18738 ;;
    b) port=18739 ;;
    c) port=18740 ;;
  esac
  tower_token=$(sudo cat "/opt/xhub-v3.6-test/secrets/wt-$tower-api-token.txt")
  for attester in a b c; do
    response_file="$artifact_dir/wt-$tower-accepts-$attester.json"
    attestation_http=$(curl --silent --show-error --max-time 30 \
      --output "$response_file" --write-out '%{http_code}' \
      --request POST "http://127.0.0.1:$port/api/v3.6/custody-attestations" \
      --header "Authorization: Bearer $tower_token" \
      --header 'x-xhub-protocol-version: 0x0360' \
      --header 'content-type: application/json' \
      --data-binary "@$artifact_dir/wt-$attester-signed-attestation.json")
    if [[ "$attestation_http" != "200" ]]; then
      cat "$response_file"
      printf '\nwatchtower=%s attester=%s attestation_http=%s\n' "$tower" "$attester" "$attestation_http" >&2
      exit 1
    fi
  done

  test_file="$artifact_dir/wt-$tower-single-vps-greenlight.json"
  test_http=$(curl --silent --show-error --max-time 30 \
    --output "$test_file" --write-out '%{http_code}' \
    "http://127.0.0.1:$port/api/v3.6/funding-coins/$funding_coin_id/states/$state_sequence/entries/$entry_index/single-vps-test-greenlight?protocol_version=0x0360&threshold=2" \
    --header "Authorization: Bearer $tower_token" \
    --header 'x-xhub-protocol-version: 0x0360')
  production_file="$artifact_dir/wt-$tower-production-greenlight.json"
  production_http=$(curl --silent --show-error --max-time 30 \
    --output "$production_file" --write-out '%{http_code}' \
    "http://127.0.0.1:$port/api/v3.6/funding-coins/$funding_coin_id/states/$state_sequence/entries/$entry_index/production-greenlight?protocol_version=0x0360&threshold=2" \
    --header "Authorization: Bearer $tower_token" \
    --header 'x-xhub-protocol-version: 0x0360')
  unset tower_token
  if [[ "$test_http" != "200" || "$production_http" != "200" ]]; then
    printf 'watchtower=%s test_http=%s production_http=%s\n' "$tower" "$test_http" "$production_http" >&2
    exit 1
  fi
done

python3 - "$artifact_dir" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
result = {"watchtowers": []}
for tower in "abc":
    test = json.loads((root / f"wt-{tower}-single-vps-greenlight.json").read_text())
    production = json.loads((root / f"wt-{tower}-production-greenlight.json").read_text())
    if not (
        test.get("merchant_delivered") is True
        and test.get("custody_attester_count", 0) >= 2
        and test.get("observed_failure_domain_count") == 1
        and test.get("failure_domain_enforced") is False
        and test.get("test_only") is True
        and test.get("test_ready") is True
        and test.get("production_ready") is False
        and production.get("production_ready") is False
    ):
        raise SystemExit(f"Watchtower {tower} greenlight invariants failed")
    result["watchtowers"].append({
        "id": f"wt-{tower}",
        "merchant_delivered": test["merchant_delivered"],
        "custody_threshold": test["custody_threshold"],
        "custody_attester_count": test["custody_attester_count"],
        "observed_failure_domain_count": test["observed_failure_domain_count"],
        "test_ready": test["test_ready"],
        "production_ready": test["production_ready"],
    })
print(json.dumps(result, separators=(",", ":")))
PY

printf 'artifact_dir=%s\n' "$artifact_dir"
