use std::{
    env, fs,
    path::{Path, PathBuf},
};

use chia_bls::SecretKey;
use chia_protocol::Coin;
use clvm_utils::tree_hash;
use clvmr::{Allocator, serde::node_from_bytes};
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{
    CanonicalDecode, RecoveryPackage, StateZero, public_key_bytes, sha256_parts,
};
use xhub_puzzles_v3_6::{ClosingCoinKind, state_zero_challenge_spend_material};
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    approval::{ApprovalStatement, DUAL_APPROVED_RECHECK_REQUIRED, SignedApproval},
    bundle::test_fee_coin,
    final_recheck::FINAL_RECHECK_VERIFIED_NO_BROADCAST,
    manifest::EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
    monitor::{ChainPeak, ClosingObservation, MonitorAction, ObservedCoin},
    preparation::OFFLINE_VERIFIED_AWAITING_APPROVAL,
};

const INPUT_SCHEMA: &str = "xhub-v3-6-mainnet-recovery-package-1";
const PREFLIGHT_SCHEMA: &str = "xhub-v3-6-rpc-preflight-1";
const OUTPUT_SCHEMA: &str = "xhub-v3-6-mainnet-state-zero-three-watchtower-pipeline-1";
const PIPELINE_TIME: u64 = 2_000_000_000;
const APPROVAL_LIFETIME: u64 = 600;
const SYNTHETIC_FEE_AMOUNT: u64 = 3;
const SYNTHETIC_FEE_MOJO: u64 = 1;

#[derive(Debug, Deserialize)]
struct RecoveryPackageReport {
    schema: String,
    protocol_version: String,
    network: String,
    funding_coin_id: String,
    funding_amount_mojo: u64,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    recovery_package_canonical_hex: String,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Deserialize)]
struct RpcPreflight {
    schema: String,
    protocol_version: String,
    network_id: String,
    peak_height: u64,
    synced: bool,
    ready: bool,
    funding_coin: PreflightFundingCoin,
}

#[derive(Debug, Deserialize)]
struct PreflightFundingCoin {
    amount: u64,
    birth_height: u64,
    puzzle_hash: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TowerPipelineReport {
    watchtower_id: String,
    database_file: String,
    monitor_action: String,
    challenge_plan_status: String,
    closing_coin_id: String,
    current_state_sequence: u64,
    latest_state_sequence: u64,
    offline_preparation_status_before_approval: String,
    preparation_id: String,
    bundle_commitment: String,
    fee_coin_id: String,
    fee_mojo: u64,
    consensus_conditions_verified: bool,
    aggregate_signature_verified: bool,
    approval_status: String,
    approver_count: u16,
    logical_approval_domain_count: u16,
    final_recheck_id: String,
    final_recheck_status: String,
    final_recheck_expires_at: u64,
    execution_manifest_id: String,
    execution_manifest_status: String,
    execution_manifest_expires_at: u64,
    raw_spend_bundle_exported: bool,
    execution_authorization_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    pipeline_binding_hash: String,
}

#[derive(Debug, Serialize)]
struct ThreeWatchtowerReport {
    schema: &'static str,
    release_status: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    mainnet_approved: bool,
    test_only: bool,
    production_ready: bool,
    physical_failure_domain_count: u16,
    synthetic_logical_approval_domains: bool,
    source_preflight_peak_height: u64,
    synthetic_chain_peak_height: u64,
    synthetic_chain_snapshot: bool,
    synthetic_closing_coin: bool,
    synthetic_fee_coin: bool,
    funding_coin_id: String,
    recovery_package_content_hash: String,
    watchtower_count: usize,
    persisted_database_count: usize,
    identical_pipeline_bindings: bool,
    raw_spend_bundle_exported: bool,
    execution_authorization_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    watchtowers: Vec<TowerPipelineReport>,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: mainnet-state-zero-three-watchtower-pipeline-v3-6 RECOVERY_PACKAGE RPC_PREFLIGHT INITIAL_BIRTH_HEIGHT CHALLENGE_DEADLINE_HEIGHT OUTPUT_DIRECTORY"
                .into(),
        );
    }
    let recovery_report = read_json::<RecoveryPackageReport>(&args[0], "RecoveryPackage report")?;
    let preflight = read_json::<RpcPreflight>(&args[1], "RPC preflight")?;
    let initial_birth_height = parse_height(&args[2], "INITIAL_BIRTH_HEIGHT")?;
    let challenge_deadline_height = parse_height(&args[3], "CHALLENGE_DEADLINE_HEIGHT")?;
    let output_directory = PathBuf::from(&args[4]);
    fs::create_dir_all(&output_directory).map_err(|error| error.to_string())?;
    let report = build_three_watchtower_report(
        recovery_report,
        preflight,
        initial_birth_height,
        challenge_deadline_height,
        &output_directory,
    )?;
    let output = output_directory.join("pipeline-report.json");
    write_json(&output, &report)?;
    println!("status=THREE_WATCHTOWER_PIPELINE_VERIFIED");
    println!("watchtowers=3");
    println!("execution_manifest_status=MANIFEST_VERIFIED_NO_BROADCAST");
    println!("raw_spend_bundle_exported=false");
    println!("broadcast_enabled=false");
    println!("chain_broadcast=false");
    println!("output={}", output.display());
    Ok(())
}

fn build_three_watchtower_report(
    recovery_report: RecoveryPackageReport,
    preflight: RpcPreflight,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
    output_directory: &Path,
) -> Result<ThreeWatchtowerReport, String> {
    let (package, package_bytes, content_hash) = validate_inputs(&recovery_report, &preflight)?;
    let observation = synthetic_state_zero_observation(
        &package,
        &preflight,
        initial_birth_height,
        challenge_deadline_height,
    )?;
    let mut watchtowers = Vec::new();
    for watchtower_id in ["wt-a", "wt-b", "wt-c"] {
        let database_file = format!("{watchtower_id}.sqlite3");
        let database_path = output_directory.join(&database_file);
        if database_path.exists() {
            return Err(format!(
                "refusing to replace existing pipeline database: {}",
                database_path.display()
            ));
        }
        let mut store = WatchtowerStore::open(&database_path).map_err(|error| error.to_string())?;
        watchtowers.push(run_pipeline(
            &mut store,
            watchtower_id,
            &database_file,
            &package,
            &package_bytes,
            &observation,
            content_hash,
        )?);
    }
    let binding = watchtowers
        .first()
        .map(|tower| tower.pipeline_binding_hash.as_str())
        .ok_or("three-watchtower pipeline produced no reports")?;
    let identical_pipeline_bindings = watchtowers
        .iter()
        .all(|tower| tower.pipeline_binding_hash == binding);
    if !identical_pipeline_bindings || watchtowers.len() != 3 {
        return Err("three Watchtowers did not persist identical pipeline bindings".into());
    }
    Ok(ThreeWatchtowerReport {
        schema: OUTPUT_SCHEMA,
        release_status: "SYNTHETIC_THREE_WATCHTOWER_PIPELINE_ONLY",
        protocol_version: "0x0360",
        network: "mainnet",
        mainnet_approved: false,
        test_only: true,
        production_ready: false,
        physical_failure_domain_count: 1,
        synthetic_logical_approval_domains: true,
        source_preflight_peak_height: preflight.peak_height,
        synthetic_chain_peak_height: initial_birth_height,
        synthetic_chain_snapshot: true,
        synthetic_closing_coin: true,
        synthetic_fee_coin: true,
        funding_coin_id: recovery_report.funding_coin_id,
        recovery_package_content_hash: recovery_report.recovery_package_content_hash,
        watchtower_count: watchtowers.len(),
        persisted_database_count: watchtowers.len(),
        identical_pipeline_bindings,
        raw_spend_bundle_exported: false,
        execution_authorization_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
        watchtowers,
    })
}

fn run_pipeline(
    store: &mut WatchtowerStore,
    watchtower_id: &str,
    database_file: &str,
    package: &RecoveryPackage,
    package_bytes: &[u8],
    observation: &ClosingObservation,
    content_hash: [u8; 32],
) -> Result<TowerPipelineReport, String> {
    store
        .accept_package(package_bytes, PIPELINE_TIME)
        .map_err(|error| error.to_string())?;
    let decision = store
        .observe_chain(
            package.funding_coin_id,
            Ok(observation.clone()),
            PIPELINE_TIME + 1,
        )
        .map_err(|error| error.to_string())?;
    if decision.action != MonitorAction::ChallengePlanned {
        return Err(format!(
            "{watchtower_id} did not create the expected challenge plan: {:?}",
            decision.action
        ));
    }
    let closing = observation
        .closing_coin
        .as_ref()
        .ok_or("synthetic observation omitted Closing Coin")?;
    let plan = store
        .challenge_plan(closing.coin_id)
        .map_err(|error| error.to_string())?
        .ok_or("challenge plan was not persisted")?;

    let fee_secret = synthetic_secret(b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_KEY", &content_hash);
    let fee = test_fee_coin(
        sha256_parts(&[b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_PARENT", &content_hash]),
        SYNTHETIC_FEE_AMOUNT,
        fee_secret,
        sha256_parts(&[b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_CHANGE", &content_hash]),
        SYNTHETIC_FEE_MOJO,
    )?;
    let bundle = store
        .prepare_offline_challenge(observation, &fee, PIPELINE_TIME + 2)
        .map_err(|error| error.to_string())?;
    let preparation = store
        .offline_preparation(closing.coin_id)
        .map_err(|error| error.to_string())?
        .ok_or("offline preparation was not persisted")?;
    if preparation.status != OFFLINE_VERIFIED_AWAITING_APPROVAL
        || preparation.bundle_commitment != bundle.commitment()
    {
        return Err("offline preparation persistence binding failed".into());
    }
    let status_before_approval = preparation.status.clone();

    let approval_expiry = PIPELINE_TIME + APPROVAL_LIFETIME;
    for (index, (approver_id, failure_domain)) in [
        ("synthetic-operator-a", "synthetic-logical-domain-a"),
        ("synthetic-operator-b", "synthetic-logical-domain-b"),
    ]
    .into_iter()
    .enumerate()
    {
        let secret = synthetic_secret(
            b"XHUB_V3_6_SYNTHETIC_CHALLENGE_APPROVER",
            &[&content_hash[..], &(index as u64).to_be_bytes()].concat(),
        );
        let statement = ApprovalStatement::for_preparation(
            &preparation,
            approver_id,
            failure_domain,
            public_key_bytes(&secret),
            PIPELINE_TIME + 2,
            approval_expiry,
            sha256_parts(&[
                b"XHUB_V3_6_SYNTHETIC_CHALLENGE_APPROVAL_NONCE",
                &content_hash,
                &(index as u64).to_be_bytes(),
            ]),
        );
        let signed = SignedApproval::sign(statement, &secret).map_err(|error| error.to_string())?;
        store
            .submit_challenge_approval(&signed, PIPELINE_TIME + 3)
            .map_err(|error| error.to_string())?;
    }
    let approval = store
        .approval_status(closing.coin_id, PIPELINE_TIME + 3)
        .map_err(|error| error.to_string())?;
    if approval.status != DUAL_APPROVED_RECHECK_REQUIRED
        || approval.approver_count != 2
        || approval.failure_domain_count != 2
    {
        return Err("synthetic dual approval gate was not satisfied".into());
    }

    let recheck = store
        .perform_final_chain_recheck(observation, PIPELINE_TIME + 4)
        .map_err(|error| error.to_string())?;
    if recheck.status != FINAL_RECHECK_VERIFIED_NO_BROADCAST
        || recheck.bundle_commitment != preparation.bundle_commitment
        || recheck.broadcast_enabled
        || recheck.broadcast_ready
        || recheck.chain_broadcast
    {
        return Err("final chain recheck did not preserve non-broadcast bindings".into());
    }
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, PIPELINE_TIME + 5)
        .map_err(|error| error.to_string())?;
    if manifest.status != EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
        || manifest.bundle_commitment != preparation.bundle_commitment
        || manifest.broadcast_enabled
        || manifest.broadcast_ready
        || manifest.chain_broadcast
    {
        return Err("execution manifest did not preserve non-broadcast bindings".into());
    }

    let binding_hash = sha256_parts(&[
        b"XHUB_V3_6_SYNTHETIC_THREE_WATCHTOWER_PIPELINE",
        &package.funding_coin_id,
        &closing.coin_id,
        &approval.preparation_id,
        &preparation.report_hash,
        &preparation.bundle_commitment,
        &recheck.approval_set_hash,
        &observation.peak.height.to_be_bytes(),
        &observation.peak.header_hash,
        &preparation.challenge_deadline_height.to_be_bytes(),
    ]);
    Ok(TowerPipelineReport {
        watchtower_id: watchtower_id.into(),
        database_file: database_file.into(),
        monitor_action: "CHALLENGE_PLANNED".into(),
        challenge_plan_status: plan.status,
        closing_coin_id: hex::encode(closing.coin_id),
        current_state_sequence: plan.current_state_sequence,
        latest_state_sequence: plan.latest_state_sequence,
        offline_preparation_status_before_approval: status_before_approval,
        preparation_id: hex::encode(approval.preparation_id),
        bundle_commitment: hex::encode(preparation.bundle_commitment),
        fee_coin_id: hex::encode(preparation.fee_coin_id),
        fee_mojo: preparation.fee_mojo,
        consensus_conditions_verified: bundle.report().consensus_conditions_verified,
        aggregate_signature_verified: bundle.report().aggregate_signature_verified,
        approval_status: approval.status,
        approver_count: approval.approver_count,
        logical_approval_domain_count: approval.failure_domain_count,
        final_recheck_id: hex::encode(recheck.recheck_id),
        final_recheck_status: recheck.status,
        final_recheck_expires_at: recheck.expires_at,
        execution_manifest_id: hex::encode(manifest.manifest_id),
        execution_manifest_status: manifest.status,
        execution_manifest_expires_at: manifest.expires_at,
        raw_spend_bundle_exported: false,
        execution_authorization_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
        pipeline_binding_hash: hex::encode(binding_hash),
    })
}

fn synthetic_state_zero_observation(
    package: &RecoveryPackage,
    preflight: &RpcPreflight,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<ClosingObservation, String> {
    let material = state_zero_challenge_spend_material(
        package,
        initial_birth_height,
        challenge_deadline_height,
    )?;
    let zero_hash = StateZero::new(&package.channel_terms)
        .and_then(|state| state.hash(&package.channel_terms, &package.funding_coin_id))
        .map_err(|error| error.to_string())?;
    let closing_coin = Coin::new(
        package.funding_coin_id.into(),
        material.expected_closing_puzzle_hash.into(),
        package.funding_amount,
    );
    let peak_header_hash = sha256_parts(&[
        b"XHUB_V3_6_SYNTHETIC_CHALLENGE_PEAK",
        &closing_coin.coin_id().to_bytes(),
    ]);
    Ok(ClosingObservation {
        network_id: package.channel_terms.network_id,
        synced: true,
        peak: ChainPeak {
            height: initial_birth_height,
            header_hash: peak_header_hash,
        },
        funding_coin: ObservedCoin {
            coin_id: package.funding_coin_id,
            parent_coin_id: [0; 32],
            puzzle_hash: decode_fixed(&preflight.funding_coin.puzzle_hash, "funding puzzle hash")?,
            amount: package.funding_amount,
            birth_height: preflight.funding_coin.birth_height,
            spent_height: Some(initial_birth_height),
        },
        closing_coin: Some(ObservedCoin {
            coin_id: closing_coin.coin_id().to_bytes(),
            parent_coin_id: package.funding_coin_id,
            puzzle_hash: material.expected_closing_puzzle_hash,
            amount: package.funding_amount,
            birth_height: initial_birth_height,
            spent_height: None,
        }),
        closing_coin_kind: Some(ClosingCoinKind::Initial),
        current_state_sequence: Some(0),
        current_checkpoint_hash: Some(zero_hash),
        initial_birth_height: Some(initial_birth_height),
        challenge_deadline_height: Some(challenge_deadline_height),
        terminal_finalized: false,
    })
}

fn validate_inputs(
    report: &RecoveryPackageReport,
    preflight: &RpcPreflight,
) -> Result<(RecoveryPackage, Vec<u8>, [u8; 32]), String> {
    if report.schema != INPUT_SCHEMA
        || report.protocol_version != "0x0360"
        || report.network != "mainnet"
        || report.funding_amount_mojo != 5
        || report.state_sequence != 1
        || report.spend_bundle_created
        || report.broadcast_enabled
        || report.chain_broadcast
    {
        return Err("input is not the non-broadcast 5-mojo state 1 RecoveryPackage".into());
    }
    if preflight.schema != PREFLIGHT_SCHEMA
        || preflight.protocol_version != "0x0360"
        || !preflight.synced
        || !preflight.ready
        || preflight.funding_coin.status != "CONFIRMED"
        || preflight.funding_coin.amount != report.funding_amount_mojo
    {
        return Err("RPC preflight is not the confirmed 5-mojo mainnet input".into());
    }
    let package_bytes = decode_hex(
        &report.recovery_package_canonical_hex,
        "RecoveryPackage canonical bytes",
    )?;
    let package = RecoveryPackage::from_canonical_bytes(&package_bytes)
        .map_err(|error| format!("invalid RecoveryPackage: {error}"))?;
    package.validate().map_err(|error| error.to_string())?;
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| error.to_string())?;
    let content_hash = package.content_hash().map_err(|error| error.to_string())?;
    let funding_puzzle_hash = puzzle_hash(&package.funding_puzzle_reveal)?;
    if hex::encode(package.funding_coin_id) != report.funding_coin_id
        || package.funding_amount != report.funding_amount_mojo
        || package.official_state.checkpoint.state_sequence != report.state_sequence
        || hex::encode(checkpoint_hash) != report.checkpoint_hash
        || hex::encode(content_hash) != report.recovery_package_content_hash
        || package.channel_terms.network_id
            != decode_fixed(&preflight.network_id, "preflight network ID")?
        || funding_puzzle_hash
            != decode_fixed(&preflight.funding_coin.puzzle_hash, "preflight puzzle hash")?
    {
        return Err("RecoveryPackage or RPC preflight binding is invalid".into());
    }
    Ok((package, package_bytes, content_hash))
}

fn synthetic_secret(domain: &[u8], binding: &[u8]) -> SecretKey {
    SecretKey::from_seed(&sha256_parts(&[domain, binding]))
}

fn puzzle_hash(bytes: &[u8]) -> Result<[u8; 32], String> {
    let mut allocator = Allocator::new();
    let node = node_from_bytes(&mut allocator, bytes)
        .map_err(|error| format!("invalid funding puzzle reveal: {error:?}"))?;
    Ok(tree_hash(&allocator, node).to_bytes())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str, name: &str) -> Result<T, String> {
    serde_json::from_str(
        &fs::read_to_string(path).map_err(|error| format!("cannot read {name}: {error}"))?,
    )
    .map_err(|error| format!("invalid {name}: {error}"))
}

fn parse_height(value: &str, field: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{field} must be a u64"))
}

fn decode_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| format!("invalid {field}: {error}"))
}

fn decode_fixed(value: &str, field: &str) -> Result<[u8; 32], String> {
    decode_hex(value, field)?
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

    const RECOVERY_REPORT: &str = include_str!(
        "../../../mainnet-experiment/three-watchtower-canary/closing-state-1/recovery-package-state-1.json"
    );
    const RPC_PREFLIGHT: &str = include_str!(
        "../../../mainnet-experiment/three-watchtower-canary/rpc-preflight-closing.json"
    );

    fn inputs() -> (RecoveryPackageReport, RpcPreflight) {
        (
            serde_json::from_str(RECOVERY_REPORT).expect("RecoveryPackage report"),
            serde_json::from_str(RPC_PREFLIGHT).expect("RPC preflight"),
        )
    }

    fn in_memory_pipeline(id: &str) -> TowerPipelineReport {
        let (report, preflight) = inputs();
        let (package, bytes, content_hash) =
            validate_inputs(&report, &preflight).expect("validated inputs");
        let observation =
            synthetic_state_zero_observation(&package, &preflight, 9_159_459, 9_165_459)
                .expect("observation");
        let mut store = WatchtowerStore::open_in_memory().expect("store");
        run_pipeline(
            &mut store,
            id,
            "memory",
            &package,
            &bytes,
            &observation,
            content_hash,
        )
        .expect("pipeline")
    }

    #[test]
    fn completes_all_three_non_broadcast_gates() {
        let tower = in_memory_pipeline("wt-a");
        assert_eq!(tower.monitor_action, "CHALLENGE_PLANNED");
        assert_eq!(tower.challenge_plan_status, "SIMULATED_ONLY");
        assert_eq!(
            tower.offline_preparation_status_before_approval,
            OFFLINE_VERIFIED_AWAITING_APPROVAL
        );
        assert_eq!(tower.approval_status, DUAL_APPROVED_RECHECK_REQUIRED);
        assert_eq!(
            tower.final_recheck_status,
            FINAL_RECHECK_VERIFIED_NO_BROADCAST
        );
        assert_eq!(
            tower.execution_manifest_status,
            EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
        );
    }

    #[test]
    fn three_watchtowers_produce_identical_pipeline_bindings() {
        let reports = [
            in_memory_pipeline("wt-a"),
            in_memory_pipeline("wt-b"),
            in_memory_pipeline("wt-c"),
        ];
        assert!(
            reports
                .iter()
                .all(|tower| tower.pipeline_binding_hash == reports[0].pipeline_binding_hash)
        );
    }

    #[test]
    fn rejects_a_wrong_challenge_deadline() {
        let (report, preflight) = inputs();
        let (package, _, _) = validate_inputs(&report, &preflight).expect("validated inputs");
        assert!(
            synthetic_state_zero_observation(&package, &preflight, 9_159_459, 9_165_458).is_err()
        );
    }

    #[test]
    fn pipeline_never_enables_broadcast_or_exports_the_bundle() {
        let tower = in_memory_pipeline("wt-a");
        assert!(tower.consensus_conditions_verified);
        assert!(tower.aggregate_signature_verified);
        assert!(!tower.raw_spend_bundle_exported);
        assert!(!tower.execution_authorization_created);
        assert!(!tower.broadcast_enabled);
        assert!(!tower.broadcast_ready);
        assert!(!tower.chain_broadcast);
    }
}
