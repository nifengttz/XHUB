#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <confirmers.json>" >&2
  exit 2
fi

source_file=$1
target_file=/opt/xhub-v3.6-test/config/confirmers.local.json
expected_public_key=a870ee2a2452db2324e2caf2f3f576edc7923d76630b2aa7f259934173c4031ebaab984681de083f2cdd05dbc5807910

python3 - "$source_file" "$expected_public_key" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    confirmers = json.load(source)
expected = [{
    "signer_id": "merchant-mainnet-experiment-1",
    "failure_domain": "local-mainnet-experiment",
    "signer_public_key": sys.argv[2],
}]
if confirmers != expected:
    raise SystemExit("confirmer configuration does not match the canary merchant identity")
PY

backup_file="$target_file.backup-$(date -u +%Y%m%dT%H%M%SZ)"
sudo cp "$target_file" "$backup_file"
sudo cp "$source_file" "$target_file"
sudo chmod 644 "$target_file"

for container in xhub-v36-wt-a xhub-v36-wt-b xhub-v36-wt-c; do
  sudo docker restart "$container" >/dev/null
done

for container in xhub-v36-wt-a xhub-v36-wt-b xhub-v36-wt-c; do
  for _ in $(seq 1 30); do
    health=$(sudo docker inspect --format '{{.State.Health.Status}}' "$container")
    if [[ "$health" == "healthy" ]]; then
      printf 'container=%s health=healthy\n' "$container"
      break
    fi
    sleep 1
  done
  if [[ "$health" != "healthy" ]]; then
    echo "$container did not become healthy" >&2
    exit 1
  fi
done

printf 'backup=%s\n' "$backup_file"
