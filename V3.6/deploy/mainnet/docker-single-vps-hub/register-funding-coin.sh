#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 <funding-candidate.json> <funding-coin-id> [hub-token-file]" >&2
  exit 2
fi

candidate_file=$1
funding_coin_id=${2#0x}
token_file=${3:-/opt/xhub-v3.6-hub-test/secrets/hub-api-token.txt}
hub_base_url=${XHUB_HUB_BASE_URL:-http://127.0.0.1:18737}

payload_file=$(mktemp)
response_file=$(mktemp)
trap 'rm -f "$payload_file" "$response_file"' EXIT

python3 - "$candidate_file" "$funding_coin_id" >"$payload_file" <<'PY'
import json
import re
import sys

candidate_path, coin_id = sys.argv[1:]
if re.fullmatch(r"[0-9a-fA-F]{64}", coin_id) is None:
    raise SystemExit("funding coin ID must be exactly 32 bytes of hex")

with open(candidate_path, "r", encoding="utf-8") as source:
    candidate = json.load(source)

if candidate.get("protocol_version") != "0x0360":
    raise SystemExit("candidate protocol_version must be 0x0360")
if candidate.get("network") != "mainnet":
    raise SystemExit("candidate network must be mainnet")

payload = {
    "protocol_version": "0x0360",
    "funding_coin_id": coin_id.lower(),
    "funding_puzzle_reveal_hex": candidate["funding_puzzle_reveal"],
    "channel_terms_canonical_hex": candidate["channel_terms_canonical_hex"],
}
json.dump(payload, sys.stdout, separators=(",", ":"))
PY

hub_token=$(sudo cat "$token_file")
http_status=$(curl --silent --show-error --max-time 180 \
  --output "$response_file" \
  --write-out '%{http_code}' \
  --request POST "$hub_base_url/api/v3.6/funding-coins" \
  --header "Authorization: Bearer $hub_token" \
  --header 'x-xhub-protocol-version: 0x0360' \
  --header 'content-type: application/json' \
  --data-binary "@$payload_file")
unset hub_token

cat "$response_file"
printf '\nHTTP_STATUS=%s\n' "$http_status"

if [[ "$http_status" != "201" ]]; then
  exit 1
fi
