#!/usr/bin/env bash
set -euo pipefail

root=/opt/xhub-v3.6-test
staging=${1:-/home/ubuntu/xhub-v36-readonly-monitor-staging}
backup="$root/deploy-backups/readonly-monitor-$(date -u +%Y%m%dT%H%M%SZ)"
overlay="$root/generated-compose.local/compose.readonly-monitors.yaml"
image=xhub-watchtower-v3-6:readonly-monitor
alert_data="$root/data/monitor-aggregate"
ack_token="$root/secrets/monitor-aggregate-api-token.txt"

for file in rpc.rs monitor.rs monitor_aggregate.rs watchtower.Cargo.toml Dockerfile compose.readonly-monitors.yaml; do
  test -f "$staging/$file"
done
test -s "$ack_token"
install -d -o 65532 -g 65532 -m 0750 "$alert_data"
chown 65532:65532 "$ack_token"
chmod 0400 "$ack_token"

install -d -m 0755 "$backup"
cp --preserve=mode,timestamps "$root/watchtower-v3_6/src/rpc.rs" "$backup/rpc.rs"
cp --preserve=mode,timestamps "$root/watchtower-v3_6/src/bin/monitor.rs" "$backup/monitor.rs"
cp --preserve=mode,timestamps "$root/watchtower-v3_6/Cargo.toml" "$backup/watchtower.Cargo.toml"
cp --preserve=mode,timestamps "$root/deploy/mainnet/docker-single-vps/Dockerfile" "$backup/Dockerfile"
if test -f "$root/watchtower-v3_6/src/bin/monitor_aggregate.rs"; then
  cp --preserve=mode,timestamps "$root/watchtower-v3_6/src/bin/monitor_aggregate.rs" "$backup/monitor_aggregate.rs"
fi
if test -f "$overlay"; then
  cp --preserve=mode,timestamps "$overlay" "$backup/compose.readonly-monitors.yaml"
fi

install -o root -g root -m 0644 "$staging/rpc.rs" "$root/watchtower-v3_6/src/rpc.rs"
install -o root -g root -m 0644 "$staging/monitor.rs" "$root/watchtower-v3_6/src/bin/monitor.rs"
install -o root -g root -m 0644 "$staging/monitor_aggregate.rs" "$root/watchtower-v3_6/src/bin/monitor_aggregate.rs"
install -o root -g root -m 0644 "$staging/watchtower.Cargo.toml" "$root/watchtower-v3_6/Cargo.toml"
install -o root -g root -m 0644 "$staging/Dockerfile" "$root/deploy/mainnet/docker-single-vps/Dockerfile"
install -o root -g root -m 0644 "$staging/compose.readonly-monitors.yaml" "$overlay"

docker compose -f "$overlay" config >/dev/null
docker build --file "$root/deploy/mainnet/docker-single-vps/Dockerfile" --tag "$image" "$root"
docker compose -f "$overlay" up -d

for attempt in $(seq 1 60); do
  healthy=true
  for container in xhub-v36-monitor-a xhub-v36-monitor-b xhub-v36-monitor-c xhub-v36-monitor-aggregate; do
    if test "$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}missing{{end}}' "$container")" != healthy; then
      healthy=false
    fi
  done
  if test "$healthy" = true; then
    break
  fi
  if test "$attempt" = 60; then
    echo "read-only monitors did not become healthy" >&2
    exit 1
  fi
  sleep 2
done
for container in xhub-v36-wt-a xhub-v36-wt-b xhub-v36-wt-c xhub-v36-hub; do
  test "$(docker inspect --format '{{.State.Running}}' "$container")" = true
done

echo "status=READONLY_MONITORS_STARTED"
echo "image=$image"
echo "backup=$backup"
echo "overlay=$overlay"
