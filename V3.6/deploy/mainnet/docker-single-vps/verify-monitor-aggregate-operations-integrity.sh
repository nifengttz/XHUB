#!/usr/bin/env bash
set -euo pipefail

root=/opt/xhub-v3.6-test
manifest=${1:-$root/operations/operations-integrity-manifest.json}

command -v jq >/dev/null
command -v sha256sum >/dev/null
test -s "$manifest"
manifest_json=$(cat "$manifest")
jq -e '
  .schema == "xhub-v3-6-monitor-aggregate-operations-integrity-1" and
  .protocol_version == "0x0360" and
  .production_ready == false and
  .chain_broadcast == false and
  (.files | length) > 0 and
  ([.files[].source_file] | unique | length) == (.files | length) and
  ([.files[].install_path] | unique | length) == (.files | length)
' <<<"$manifest_json" >/dev/null

records=$(
  while IFS=$'\t' read -r source_file install_path expected_sha256; do
    if [[ -z "$source_file" ]]; then
      echo "integrity entry omitted source_file" >&2
      exit 1
    fi
    case "$install_path" in
      "$root"/operations/* | /etc/systemd/system/xhub-v36-monitor-*.service | /etc/systemd/system/xhub-v36-monitor-*.timer) ;;
      *) echo "unmanaged integrity path: $install_path" >&2; exit 1 ;;
    esac
    if [[ ! -f "$install_path" ]]; then
      echo "integrity file is missing: $install_path" >&2
      exit 1
    fi
    actual_sha256=$(sha256sum "$install_path" | awk '{print $1}')
    if [[ "$actual_sha256" != "$expected_sha256" ]]; then
      echo "integrity hash mismatch: $install_path" >&2
      exit 1
    fi
    jq -nc \
      --arg source_file "$source_file" \
      --arg install_path "$install_path" \
      --arg sha256 "$actual_sha256" \
      '{source_file:$source_file,install_path:$install_path,sha256:$sha256,match:true}'
  done < <(jq -r '.files[] | [.source_file,.install_path,.sha256] | @tsv' <<<"$manifest_json")
)
files=$(jq -s '.' <<<"$records")
manifest_sha256=$(sha256sum "$manifest" | awk '{print $1}')

jq -n \
  --arg observed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg manifest_sha256 "$manifest_sha256" \
  --argjson files "$files" \
  '{
    schema:"xhub-v3-6-monitor-aggregate-operations-integrity-evidence-1",
    protocol_version:"0x0360",
    observed_at:$observed_at,
    status:"PASS",
    manifest_sha256:$manifest_sha256,
    verified_file_count:($files | length),
    all_files_match:true,
    files:$files,
    automatic_repair_enabled:false,
    external_notification_enabled:false,
    physical_failure_domain_count:1,
    production_ready:false,
    spend_bundle_created:false,
    broadcast_enabled:false,
    broadcast_ready:false,
    chain_broadcast:false
  }'
