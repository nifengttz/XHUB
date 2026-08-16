#!/usr/bin/env bash
set -euo pipefail

root=/opt/xhub-v3.6-test
manifest="$root/operations/operations-integrity-manifest.json"
verifier="$root/operations/verify-monitor-aggregate-operations-integrity.sh"
temporary=$(mktemp)

cleanup() {
  rm -f "$temporary"
}
trap cleanup EXIT

command -v jq >/dev/null
test -s "$manifest"
test -x "$verifier"
jq '.files[0].sha256 = ("00" * 32)' "$manifest" >"$temporary"
if "$verifier" "$temporary" >/dev/null 2>&1; then
  echo "integrity verifier accepted a modified hash" >&2
  exit 1
fi

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg source_file "$(jq -r '.files[0].source_file' "$manifest")" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-operations-integrity-rejection-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    modified_manifest_copy_rejected:true,
    modified_source_file:$source_file,
    installed_files_modified:false,
    installed_manifest_modified:false,
    temporary_manifest_removed:true,
    automatic_repair_enabled:false,
    external_notification_enabled:false,
    physical_failure_domain_count:1,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
