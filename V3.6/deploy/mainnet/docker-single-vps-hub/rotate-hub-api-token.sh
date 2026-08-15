#!/usr/bin/env bash
set -euo pipefail

token_file=${1:-/opt/xhub-v3.6-hub-test/secrets/hub-api-token.txt}
container_name=${2:-xhub-v36-hub}

new_token=$(openssl rand -hex 32)
printf '%s\n' "$new_token" | sudo tee "$token_file" >/dev/null
unset new_token
sudo chmod 600 "$token_file"

sudo docker restart "$container_name" >/dev/null
for _ in $(seq 1 30); do
  health=$(sudo docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$container_name")
  if [[ "$health" == "healthy" ]]; then
    printf 'container=%s health=healthy\n' "$container_name"
    exit 0
  fi
  sleep 1
done

echo "container did not become healthy after API token rotation" >&2
exit 1
