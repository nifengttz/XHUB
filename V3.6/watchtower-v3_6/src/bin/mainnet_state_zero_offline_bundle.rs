use std::{env, fs, path::PathBuf};

use chia_bls::SecretKey;
use chia_protocol::Coin;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage, sha256_parts};
use xhub_puzzles_v3_6::{ClosingCoinKind, state_zero_challenge_spend_material};
use xhub_watchtower_v3_6::bundle::{
    ChainSnapshot, OfflineBundleReport, build_offline_challenge_bundle, test_fee_coin,
};

const INPUT_SCHEMA: &str = "xhub-v3-6-mainnet-recovery-package-1";
const OUTPUT_SCHEMA: &str = "xhub-v3-6-mainnet-state-zero-offline-bundle-1";
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

#[derive(Debug, Serialize)]
struct OfflineCanaryReport {
    schema: &'static str,
    release_status: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    mainnet_approved: bool,
    test_only: bool,
    synthetic_chain_snapshot: bool,
    synthetic_closing_coin: bool,
    synthetic_fee_coin: bool,
    raw_spend_bundle_exported: bool,
    funding_coin_id: String,
    recovery_package_content_hash: String,
    closing_parent_coin_id: String,
    construction_peak_height: u64,
    challenge_deadline_height: u64,
    bundle_commitment: String,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    bundle: OfflineBundleReport,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: mainnet-state-zero-offline-bundle-v3-6 RECOVERY_PACKAGE INITIAL_BIRTH_HEIGHT CHALLENGE_DEADLINE_HEIGHT OUTPUT_JSON"
                .into(),
        );
    }
    let report = read_report(&args[0])?;
    let initial_birth_height = parse_height(&args[1], "INITIAL_BIRTH_HEIGHT")?;
    let challenge_deadline_height = parse_height(&args[2], "CHALLENGE_DEADLINE_HEIGHT")?;
    let output = build_report(report, initial_birth_height, challenge_deadline_height)?;
    write_json(PathBuf::from(&args[3]), &output)?;
    println!("status=OFFLINE_CHALLENGE_BUNDLE_VERIFIED");
    println!("spend_bundle_created=true");
    println!("raw_spend_bundle_exported=false");
    println!("broadcast_enabled=false");
    println!("chain_broadcast=false");
    println!("output={}", args[3]);
    Ok(())
}

fn read_report(path: &str) -> Result<RecoveryPackageReport, String> {
    serde_json::from_str(
        &fs::read_to_string(path)
            .map_err(|error| format!("cannot read RecoveryPackage report: {error}"))?,
    )
    .map_err(|error| format!("invalid RecoveryPackage report: {error}"))
}

fn parse_height(value: &str, field: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{field} must be a u64"))
}

fn build_report(
    report: RecoveryPackageReport,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<OfflineCanaryReport, String> {
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

    let bytes = decode_hex(
        &report.recovery_package_canonical_hex,
        "RecoveryPackage canonical bytes",
    )?;
    let package = RecoveryPackage::from_canonical_bytes(&bytes)
        .map_err(|error| format!("invalid RecoveryPackage: {error}"))?;
    package.validate().map_err(|error| error.to_string())?;
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| error.to_string())?;
    let content_hash = package.content_hash().map_err(|error| error.to_string())?;
    if hex::encode(package.funding_coin_id) != report.funding_coin_id
        || package.funding_amount != report.funding_amount_mojo
        || package.official_state.checkpoint.state_sequence != report.state_sequence
        || hex::encode(checkpoint_hash) != report.checkpoint_hash
        || hex::encode(content_hash) != report.recovery_package_content_hash
    {
        return Err("RecoveryPackage report binding is invalid".into());
    }

    let material = state_zero_challenge_spend_material(
        &package,
        initial_birth_height,
        challenge_deadline_height,
    )?;
    let closing_coin = Coin::new(
        package.funding_coin_id.into(),
        material.expected_closing_puzzle_hash.into(),
        package.funding_amount,
    );
    let closing_coin_id = closing_coin.coin_id().to_bytes();
    let snapshot = ChainSnapshot {
        peak_height: initial_birth_height,
        peak_header_hash: sha256_parts(&[b"XHUB_V3_6_SYNTHETIC_CHALLENGE_PEAK", &closing_coin_id]),
        closing_coin_id,
        closing_coin,
        closing_birth_height: initial_birth_height,
        closing_spent_height: None,
    };
    let fee_secret = SecretKey::from_seed(&sha256_parts(&[
        b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_KEY",
        &content_hash,
    ]));
    let fee = test_fee_coin(
        sha256_parts(&[b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_PARENT", &content_hash]),
        SYNTHETIC_FEE_AMOUNT,
        fee_secret,
        sha256_parts(&[b"XHUB_V3_6_SYNTHETIC_CHALLENGE_FEE_CHANGE", &content_hash]),
        SYNTHETIC_FEE_MOJO,
    )?;
    let bundle = build_offline_challenge_bundle(
        None,
        &package,
        ClosingCoinKind::Initial,
        initial_birth_height,
        challenge_deadline_height,
        snapshot.clone(),
        &fee,
    )?;
    bundle.validate_pre_broadcast_snapshot(&snapshot)?;
    let bundle_report = bundle.report().clone();
    if !bundle_report.consensus_conditions_verified
        || !bundle_report.aggregate_signature_verified
        || !bundle_report.spend_bundle_created
        || bundle_report.broadcast_enabled
        || bundle_report.broadcast_ready
        || bundle_report.chain_broadcast
        || bundle_report.fee_mojo != SYNTHETIC_FEE_MOJO
        || bundle_report.removal_amount_mojo
            != u128::from(package.funding_amount + SYNTHETIC_FEE_AMOUNT)
        || bundle_report.addition_amount_mojo
            != u128::from(package.funding_amount + SYNTHETIC_FEE_AMOUNT - SYNTHETIC_FEE_MOJO)
    {
        return Err("offline bundle did not preserve consensus or non-broadcast gates".into());
    }

    Ok(OfflineCanaryReport {
        schema: OUTPUT_SCHEMA,
        release_status: "SYNTHETIC_OFFLINE_BUNDLE_ONLY",
        protocol_version: "0x0360",
        network: "mainnet",
        mainnet_approved: false,
        test_only: true,
        synthetic_chain_snapshot: true,
        synthetic_closing_coin: true,
        synthetic_fee_coin: true,
        raw_spend_bundle_exported: false,
        funding_coin_id: report.funding_coin_id.clone(),
        recovery_package_content_hash: report.recovery_package_content_hash,
        closing_parent_coin_id: report.funding_coin_id,
        construction_peak_height: snapshot.peak_height,
        challenge_deadline_height,
        bundle_commitment: hex::encode(bundle.commitment()),
        spend_bundle_created: true,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
        bundle: bundle_report,
    })
}

fn decode_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| format!("invalid {field}: {error}"))
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECOVERY_REPORT: &str = include_str!(
        "../../../mainnet-experiment/three-watchtower-canary/closing-state-1/recovery-package-state-1.json"
    );

    fn report() -> RecoveryPackageReport {
        serde_json::from_str(RECOVERY_REPORT).expect("checked RecoveryPackage report")
    }

    #[test]
    fn builds_and_fully_verifies_the_offline_bundle() {
        let output = build_report(report(), 9_159_459, 9_165_459).expect("offline bundle");
        assert!(output.bundle.consensus_conditions_verified);
        assert!(output.bundle.aggregate_signature_verified);
        assert_eq!(output.bundle.removal_amount_mojo, 8);
        assert_eq!(output.bundle.addition_amount_mojo, 7);
        assert_eq!(output.bundle.fee_mojo, 1);
    }

    #[test]
    fn bundle_commitment_is_stable_for_the_bound_inputs() {
        let first = build_report(report(), 9_159_459, 9_165_459).expect("first bundle");
        let second = build_report(report(), 9_159_459, 9_165_459).expect("second bundle");
        assert_eq!(first.bundle_commitment, second.bundle_commitment);
        assert_ne!(first.bundle_commitment, hex::encode([0; 32]));
    }

    #[test]
    fn rejects_a_wrong_deadline_or_state_binding() {
        assert!(build_report(report(), 9_159_459, 9_165_458).is_err());
        let mut input = report();
        input.state_sequence = 0;
        assert!(build_report(input, 9_159_459, 9_165_459).is_err());
    }

    #[test]
    fn exports_only_a_non_broadcast_commitment() {
        let output = build_report(report(), 9_159_459, 9_165_459).expect("offline bundle");
        assert!(output.test_only);
        assert!(output.synthetic_chain_snapshot);
        assert!(output.synthetic_closing_coin);
        assert!(output.synthetic_fee_coin);
        assert!(output.spend_bundle_created);
        assert!(!output.mainnet_approved);
        assert!(!output.raw_spend_bundle_exported);
        assert!(!output.broadcast_enabled);
        assert!(!output.broadcast_ready);
        assert!(!output.chain_broadcast);
    }
}
