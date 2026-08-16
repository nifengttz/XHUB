#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <verified-hub-reservation-request.json>" >&2
  exit 2
fi

request_file=$1
hub_base_url=${XHUB_HUB_BASE_URL:-http://127.0.0.1:18737}
hub_token_file=${XHUB_HUB_TOKEN_FILE:-/opt/xhub-v3.6-hub-test/secrets/hub-api-token.txt}
artifact_dir=${XHUB_CANARY_ARTIFACT_DIR:-/home/ubuntu/xhub-v36-canary-artifacts}
mkdir -p "$artifact_dir"

funding_coin_id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["funding_coin_id"])' "$request_file")
reservation_nonce=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["reservation_nonce"])' "$request_file")
run_prefix="$artifact_dir/$funding_coin_id-$reservation_nonce"
reservation_file="$run_prefix-reservation.json"
package_file="$run_prefix-recovery-package.json"
quorum_file="$run_prefix-quorum.json"

hub_token=$(sudo cat "$hub_token_file")
reservation_http=$(curl --silent --show-error --max-time 180 \
  --output "$reservation_file" --write-out '%{http_code}' \
  --request POST "$hub_base_url/api/v3.6/reservations" \
  --header "Authorization: Bearer $hub_token" \
  --header 'x-xhub-protocol-version: 0x0360' \
  --header 'content-type: application/json' \
  --data-binary "@$request_file")
if [[ "$reservation_http" != "200" ]]; then
  cat "$reservation_file"
  printf '\nreservation_http=%s\n' "$reservation_http" >&2
  exit 1
fi

state_sequence=$(python3 - "$reservation_file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    response = json.load(source)
if response.get("status") != "SIGNED" or response.get("ledger_written") is not True:
    raise SystemExit("HUB did not persist a SIGNED reservation")
sequence = response.get("state_sequence")
if not isinstance(sequence, int) or sequence < 1:
    raise SystemExit("HUB reservation has no valid state_sequence")
print(sequence)
PY
)

package_http=$(curl --silent --show-error --max-time 60 \
  --output "$package_file" --write-out '%{http_code}' \
  "$hub_base_url/api/v3.6/funding-coins/$funding_coin_id/recovery-packages/$state_sequence?protocol_version=0x0360" \
  --header "Authorization: Bearer $hub_token" \
  --header 'x-xhub-protocol-version: 0x0360')
if [[ "$package_http" != "200" ]]; then
  cat "$package_file"
  printf '\npackage_http=%s\n' "$package_http" >&2
  exit 1
fi

idempotency_key="v36-mainnet-5mojo-state-$state_sequence-three-watchtowers"
quorum_body=$(printf '{"protocol_version":"0x0360","idempotency_key":"%s"}' "$idempotency_key")
quorum_http=$(curl --silent --show-error --max-time 90 \
  --output "$quorum_file" --write-out '%{http_code}' \
  --request POST "$hub_base_url/api/v3.6/funding-coins/$funding_coin_id/recovery-packages/$state_sequence/watchtower-quorum-deliveries" \
  --header "Authorization: Bearer $hub_token" \
  --header 'x-xhub-protocol-version: 0x0360' \
  --header 'content-type: application/json' \
  --data-binary "$quorum_body")
unset hub_token
if [[ "$quorum_http" != "200" ]]; then
  cat "$quorum_file"
  printf '\nquorum_http=%s\n' "$quorum_http" >&2
  exit 1
fi

python3 - "$reservation_file" "$package_file" "$quorum_file" <<'PY'
import json
import sys

reservation, package, quorum = [json.load(open(path, encoding="utf-8")) for path in sys.argv[1:]]
if reservation.get("recovery_package_content_hash") != package.get("recovery_package_content_hash"):
    raise SystemExit("reservation and RecoveryPackage content hashes differ")
if quorum.get("quorum_met") is not True or quorum.get("delivered_count", 0) < 2:
    raise SystemExit("Watchtower delivery quorum was not met")
print(json.dumps({
    "reservation": {
        "status": reservation.get("status"),
        "ledger_written": reservation.get("ledger_written"),
        "state_sequence": reservation.get("state_sequence"),
        "checkpoint_hash": reservation.get("checkpoint_hash"),
        "recovery_package_content_hash": reservation.get("recovery_package_content_hash"),
    },
    "quorum": {
        "configured_recipient_count": quorum.get("configured_recipient_count"),
        "quorum_required": quorum.get("quorum_required"),
        "delivered_count": quorum.get("delivered_count"),
        "quorum_met": quorum.get("quorum_met"),
        "deliveries": [
            {"recipient_id": delivery.get("recipient_id"), "status": delivery.get("status")}
            for delivery in quorum.get("deliveries", [])
        ],
    },
}, separators=(",", ":")))
PY

for tower in a b c; do
  case "$tower" in
    a) port=18738 ;;
    b) port=18739 ;;
    c) port=18740 ;;
  esac
  tower_token=$(sudo cat "/opt/xhub-v3.6-test/secrets/wt-$tower-api-token.txt")
  tower_file="$run_prefix-wt-$tower-package.json"
  tower_http=$(curl --silent --show-error --max-time 30 \
    --output "$tower_file" --write-out '%{http_code}' \
    "http://127.0.0.1:$port/api/v3.6/funding-coins/$funding_coin_id/recovery-packages/$state_sequence?protocol_version=0x0360" \
    --header "Authorization: Bearer $tower_token" \
    --header 'x-xhub-protocol-version: 0x0360')
  unset tower_token
  if [[ "$tower_http" != "200" ]]; then
    printf 'watchtower=%s http=%s persisted=false\n' "$tower" "$tower_http" >&2
    exit 1
  fi
  python3 - "$tower" "$package_file" "$tower_file" <<'PY'
import json
import sys

tower = sys.argv[1]
hub = json.load(open(sys.argv[2], encoding="utf-8"))
stored = json.load(open(sys.argv[3], encoding="utf-8"))
matches = (
    stored.get("recovery_package_content_hash") == hub.get("recovery_package_content_hash")
    and stored.get("recovery_package_canonical_hex") == hub.get("recovery_package_canonical_hex")
)
if not matches:
    raise SystemExit(f"Watchtower {tower} persisted a mismatched RecoveryPackage")
print(f"watchtower={tower} persisted=true package_matches=true")
PY
done

printf 'artifact_prefix=%s\n' "$run_prefix"
