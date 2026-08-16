use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::sha256_parts;
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    authorization::{
        EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY, EXECUTION_AUTHORIZED_SIMULATED_ONLY,
        SIMULATED_SUBMISSION_RECORDED,
    },
    manifest::EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
};

const INPUT_SCHEMA: &str = "xhub-v3-6-mainnet-state-zero-three-watchtower-pipeline-1";
const OUTPUT_SCHEMA: &str = "xhub-v3-6-mainnet-state-zero-execution-dry-run-1";
const PIPELINE_TIME: u64 = 2_000_000_000;

#[derive(Debug, Deserialize)]
struct PipelineReport {
    schema: String,
    mainnet_approved: bool,
    test_only: bool,
    production_ready: bool,
    synthetic_chain_snapshot: bool,
    synthetic_closing_coin: bool,
    synthetic_fee_coin: bool,
    watchtower_count: usize,
    persisted_database_count: usize,
    identical_pipeline_bindings: bool,
    raw_spend_bundle_exported: bool,
    execution_authorization_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    watchtowers: Vec<PipelineTower>,
}

#[derive(Debug, Deserialize)]
struct PipelineTower {
    watchtower_id: String,
    database_file: String,
    bundle_commitment: String,
    execution_manifest_id: String,
    execution_manifest_status: String,
    pipeline_binding_hash: String,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TowerDryRunReport {
    watchtower_id: String,
    database_file: String,
    adapter_mode: &'static str,
    authorization_id: String,
    authorization_status_before_submission: String,
    authorization_expires_at: u64,
    authorization_status_after_submission: String,
    simulated_submission_count: u64,
    submission_receipt_id: String,
    submission_receipt_status: String,
    submission_nonce: String,
    idempotent_replay_verified: bool,
    conflicting_replay_rejected: bool,
    reauthorization_rejected: bool,
    bundle_commitment: String,
    audit_event_count: u64,
    audit_head_hash: String,
    audit_chain_valid: bool,
    audit_anchor_id: String,
    audit_anchor_valid: bool,
    audit_rollback_detected: bool,
    rpc_request_created: bool,
    rpc_called: bool,
    push_tx_called: bool,
    raw_spend_bundle_present: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    dry_run_binding_hash: String,
}

#[derive(Debug, Serialize)]
struct ExecutionDryRunReport {
    schema: &'static str,
    release_status: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    adapter_mode: &'static str,
    mainnet_approved: bool,
    test_only: bool,
    production_ready: bool,
    physical_failure_domain_count: u16,
    synthetic_chain_snapshot: bool,
    synthetic_closing_coin: bool,
    synthetic_fee_coin: bool,
    watchtower_count: usize,
    authorization_count: usize,
    simulated_receipt_count: usize,
    identical_dry_run_bindings: bool,
    all_audit_chains_valid: bool,
    all_one_time_authorizations_consumed: bool,
    all_conflicting_replays_rejected: bool,
    rpc_request_created: bool,
    rpc_called: bool,
    push_tx_called: bool,
    raw_spend_bundle_present: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    watchtowers: Vec<TowerDryRunReport>,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(
            "usage: mainnet-state-zero-execution-dry-run-v3-6 PIPELINE_REPORT PIPELINE_DIRECTORY OUTPUT_JSON"
                .into(),
        );
    }
    let pipeline: PipelineReport = read_json(&args[0], "three-Watchtower pipeline report")?;
    let output = PathBuf::from(&args[2]);
    if output.exists() {
        return Err(format!(
            "refusing to replace existing dry-run report: {}",
            output.display()
        ));
    }
    let report = execute_dry_run(pipeline, Path::new(&args[1]))?;
    write_json(&output, &report)?;
    println!("status=EXECUTION_DRY_RUN_RECORDED");
    println!("authorizations=3");
    println!("simulated_receipts=3");
    println!("audit_chains_valid=true");
    println!("rpc_called=false");
    println!("push_tx_called=false");
    println!("chain_broadcast=false");
    println!("output={}", output.display());
    Ok(())
}

fn execute_dry_run(
    pipeline: PipelineReport,
    pipeline_directory: &Path,
) -> Result<ExecutionDryRunReport, String> {
    validate_pipeline_report(&pipeline)?;
    let mut watchtowers = Vec::with_capacity(pipeline.watchtowers.len());
    for tower in &pipeline.watchtowers {
        let database_path = checked_database_path(pipeline_directory, &tower.database_file)?;
        if !database_path.is_file() {
            return Err(format!(
                "Watchtower database was not found: {}",
                database_path.display()
            ));
        }
        let store = WatchtowerStore::open(&database_path).map_err(|error| error.to_string())?;
        watchtowers.push(execute_tower_dry_run(&store, tower)?);
    }
    let first_binding = watchtowers
        .first()
        .map(|tower| tower.dry_run_binding_hash.as_str())
        .ok_or("execution dry-run produced no Watchtower reports")?;
    let identical_dry_run_bindings = watchtowers
        .iter()
        .all(|tower| tower.dry_run_binding_hash == first_binding);
    let all_audit_chains_valid = watchtowers.iter().all(|tower| tower.audit_chain_valid);
    let all_one_time_authorizations_consumed = watchtowers.iter().all(|tower| {
        tower.authorization_status_after_submission
            == EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY
            && tower.simulated_submission_count == 1
    });
    let all_conflicting_replays_rejected = watchtowers
        .iter()
        .all(|tower| tower.conflicting_replay_rejected && tower.reauthorization_rejected);
    if !identical_dry_run_bindings
        || !all_audit_chains_valid
        || !all_one_time_authorizations_consumed
        || !all_conflicting_replays_rejected
    {
        return Err("three-Watchtower execution dry-run bindings diverged".into());
    }
    Ok(ExecutionDryRunReport {
        schema: OUTPUT_SCHEMA,
        release_status: "SIMULATED_EXECUTION_RECEIPTS_ONLY",
        protocol_version: "0x0360",
        network: "mainnet",
        adapter_mode: "NO_RPC_SIMULATED_SUBMISSION",
        mainnet_approved: false,
        test_only: true,
        production_ready: false,
        physical_failure_domain_count: 1,
        synthetic_chain_snapshot: true,
        synthetic_closing_coin: true,
        synthetic_fee_coin: true,
        watchtower_count: watchtowers.len(),
        authorization_count: watchtowers.len(),
        simulated_receipt_count: watchtowers.len(),
        identical_dry_run_bindings,
        all_audit_chains_valid,
        all_one_time_authorizations_consumed,
        all_conflicting_replays_rejected,
        rpc_request_created: false,
        rpc_called: false,
        push_tx_called: false,
        raw_spend_bundle_present: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
        watchtowers,
    })
}

fn execute_tower_dry_run(
    store: &WatchtowerStore,
    tower: &PipelineTower,
) -> Result<TowerDryRunReport, String> {
    let manifest_id = decode_fixed(&tower.execution_manifest_id, "execution manifest ID")?;
    let expected_bundle = decode_fixed(&tower.bundle_commitment, "bundle commitment")?;
    let manifest = store
        .execution_manifest(manifest_id, PIPELINE_TIME + 6)
        .map_err(|error| error.to_string())?
        .ok_or("execution manifest was not persisted")?;
    if manifest.status != EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
        || manifest.bundle_commitment != expected_bundle
        || manifest.broadcast_enabled
        || manifest.broadcast_ready
        || manifest.chain_broadcast
    {
        return Err("persisted manifest is not eligible for a dry-run authorization".into());
    }
    let authorization = store
        .issue_execution_authorization(manifest_id, PIPELINE_TIME + 6)
        .map_err(|error| error.to_string())?;
    if authorization.status != EXECUTION_AUTHORIZED_SIMULATED_ONLY
        || authorization.bundle_commitment != expected_bundle
        || authorization.simulated_submission_count != 0
        || authorization.broadcast_enabled
        || authorization.broadcast_ready
        || authorization.chain_broadcast
    {
        return Err("execution authorization did not preserve dry-run bindings".into());
    }
    let submission_nonce = sha256_parts(&[
        b"XHUB_V3_6_MAINNET_STATE_ZERO_DRY_RUN_NONCE",
        &manifest_id,
        &expected_bundle,
    ]);
    let receipt = store
        .simulate_execution_submission(
            authorization.authorization_id,
            submission_nonce,
            PIPELINE_TIME + 7,
        )
        .map_err(|error| error.to_string())?;
    if receipt.status != SIMULATED_SUBMISSION_RECORDED
        || receipt.bundle_commitment != expected_bundle
        || receipt.broadcast_enabled
        || receipt.broadcast_ready
        || receipt.chain_broadcast
    {
        return Err("simulated submission receipt did not preserve dry-run bindings".into());
    }
    let idempotent = store
        .simulate_execution_submission(
            authorization.authorization_id,
            submission_nonce,
            PIPELINE_TIME + 8,
        )
        .map_err(|error| error.to_string())?;
    let idempotent_replay_verified = idempotent == receipt;
    let conflicting_nonce = sha256_parts(&[
        b"XHUB_V3_6_MAINNET_STATE_ZERO_DRY_RUN_CONFLICT",
        &manifest_id,
        &expected_bundle,
    ]);
    let conflicting_replay_rejected = store
        .simulate_execution_submission(
            authorization.authorization_id,
            conflicting_nonce,
            PIPELINE_TIME + 8,
        )
        .is_err();
    let reauthorization_rejected = store
        .issue_execution_authorization(manifest_id, PIPELINE_TIME + 8)
        .is_err();
    let consumed = store
        .execution_authorization(authorization.authorization_id, PIPELINE_TIME + 8)
        .map_err(|error| error.to_string())?
        .ok_or("consumed execution authorization disappeared")?;
    let audit = store
        .verify_execution_audit_chain()
        .map_err(|error| error.to_string())?;
    let anchor = store
        .create_execution_audit_anchor(PIPELINE_TIME + 8)
        .map_err(|error| error.to_string())?;
    let anchor_check = store
        .verify_execution_audit_anchor(&anchor)
        .map_err(|error| error.to_string())?;
    if !idempotent_replay_verified
        || !conflicting_replay_rejected
        || !reauthorization_rejected
        || consumed.status != EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY
        || consumed.simulated_submission_count != 1
        || consumed.last_simulated_at != Some(PIPELINE_TIME + 7)
        || !audit.valid
        || audit.head.event_count != 3
        || !anchor_check.valid
        || anchor_check.rollback_detected
    {
        return Err("one-time authorization, replay, or audit-chain gate failed".into());
    }
    let dry_run_binding_hash = sha256_parts(&[
        b"XHUB_V3_6_MAINNET_STATE_ZERO_EXECUTION_DRY_RUN",
        &authorization.authorization_id,
        &receipt.receipt_id,
        &expected_bundle,
        &submission_nonce,
        &audit.head.head_hash,
        &anchor.anchor_id,
    ]);
    Ok(TowerDryRunReport {
        watchtower_id: tower.watchtower_id.clone(),
        database_file: tower.database_file.clone(),
        adapter_mode: "NO_RPC_SIMULATED_SUBMISSION",
        authorization_id: hex::encode(authorization.authorization_id),
        authorization_status_before_submission: authorization.status,
        authorization_expires_at: authorization.expires_at,
        authorization_status_after_submission: consumed.status,
        simulated_submission_count: consumed.simulated_submission_count,
        submission_receipt_id: hex::encode(receipt.receipt_id),
        submission_receipt_status: receipt.status,
        submission_nonce: hex::encode(submission_nonce),
        idempotent_replay_verified,
        conflicting_replay_rejected,
        reauthorization_rejected,
        bundle_commitment: tower.bundle_commitment.clone(),
        audit_event_count: audit.head.event_count,
        audit_head_hash: hex::encode(audit.head.head_hash),
        audit_chain_valid: audit.valid,
        audit_anchor_id: hex::encode(anchor.anchor_id),
        audit_anchor_valid: anchor_check.valid,
        audit_rollback_detected: anchor_check.rollback_detected,
        rpc_request_created: false,
        rpc_called: false,
        push_tx_called: false,
        raw_spend_bundle_present: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
        dry_run_binding_hash: hex::encode(dry_run_binding_hash),
    })
}

fn validate_pipeline_report(report: &PipelineReport) -> Result<(), String> {
    if report.schema != INPUT_SCHEMA
        || report.mainnet_approved
        || !report.test_only
        || report.production_ready
        || !report.synthetic_chain_snapshot
        || !report.synthetic_closing_coin
        || !report.synthetic_fee_coin
        || report.watchtower_count != 3
        || report.persisted_database_count != 3
        || report.watchtowers.len() != 3
        || !report.identical_pipeline_bindings
        || report.raw_spend_bundle_exported
        || report.execution_authorization_created
        || report.broadcast_enabled
        || report.broadcast_ready
        || report.chain_broadcast
    {
        return Err("input is not the safe three-Watchtower non-broadcast pipeline report".into());
    }
    let mut ids = HashSet::new();
    let mut database_files = HashSet::new();
    let expected_binding = report
        .watchtowers
        .first()
        .map(|tower| tower.pipeline_binding_hash.as_str())
        .ok_or("pipeline report omitted Watchtowers")?;
    for tower in &report.watchtowers {
        if !ids.insert(&tower.watchtower_id)
            || !database_files.insert(&tower.database_file)
            || tower.execution_manifest_status != EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
            || tower.pipeline_binding_hash != expected_binding
            || tower.broadcast_enabled
            || tower.broadcast_ready
            || tower.chain_broadcast
        {
            return Err("pipeline Watchtower bindings are invalid or duplicated".into());
        }
        decode_fixed(&tower.execution_manifest_id, "execution manifest ID")?;
        decode_fixed(&tower.bundle_commitment, "bundle commitment")?;
    }
    Ok(())
}

fn checked_database_path(directory: &Path, file: &str) -> Result<PathBuf, String> {
    let relative = Path::new(file);
    if relative.components().count() != 1
        || relative.file_name().and_then(|name| name.to_str()) != Some(file)
        || relative.extension().and_then(|value| value.to_str()) != Some("sqlite3")
    {
        return Err("pipeline database file must be a plain .sqlite3 filename".into());
    }
    Ok(directory.join(relative))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str, name: &str) -> Result<T, String> {
    serde_json::from_str(
        &fs::read_to_string(path).map_err(|error| format!("cannot read {name}: {error}"))?,
    )
    .map_err(|error| format!("invalid {name}: {error}"))
}

fn decode_fixed(value: &str, field: &str) -> Result<[u8; 32], String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| format!("invalid {field}: {error}"))?
        .try_into()
        .map_err(|_| format!("{field} must be 32 bytes"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_pipeline_flags() {
        let mut report: PipelineReport = serde_json::from_str(include_str!(
            "../../../mainnet-experiment/three-watchtower-canary/closing-state-1/state-zero-pipeline/pipeline-report.json"
        ))
        .expect("pipeline report");
        report.broadcast_enabled = true;
        assert!(validate_pipeline_report(&report).is_err());
    }

    #[test]
    fn rejects_database_path_traversal() {
        assert!(checked_database_path(Path::new("pipeline"), "../wt-a.sqlite3").is_err());
        assert!(checked_database_path(Path::new("pipeline"), "wt-a.sqlite3").is_ok());
    }
}
