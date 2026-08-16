use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage};
use xhub_puzzles_v3_6::{ChallengeSimulation, ClosingCoinKind, simulate_state_zero_challenge};

const INPUT_SCHEMA: &str = "xhub-v3-6-mainnet-recovery-package-1";
const OUTPUT_SCHEMA: &str = "xhub-v3-6-mainnet-state-zero-challenge-simulation-1";

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
struct StateZeroChallengeReport {
    schema: &'static str,
    release_status: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    mainnet_approved: bool,
    test_only: bool,
    recovery_package_content_hash: String,
    spend_bundle_created: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
    simulation: ChallengeSimulation,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: mainnet-state-zero-challenge-v3-6 RECOVERY_PACKAGE INITIAL_BIRTH_HEIGHT CHALLENGE_DEADLINE_HEIGHT OUTPUT_JSON"
                .into(),
        );
    }
    let report = read_report(&args[0])?;
    let initial_birth_height = parse_height(&args[1], "INITIAL_BIRTH_HEIGHT")?;
    let challenge_deadline_height = parse_height(&args[2], "CHALLENGE_DEADLINE_HEIGHT")?;
    let output = build_report(report, initial_birth_height, challenge_deadline_height)?;
    write_json(PathBuf::from(&args[3]), &output)?;
    println!("status=STATE_ZERO_CHALLENGE_SIMULATED");
    println!("transition=INITIAL_0_TO_1");
    println!("spend_bundle_created=false");
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
) -> Result<StateZeroChallengeReport, String> {
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

    let simulation =
        simulate_state_zero_challenge(&package, initial_birth_height, challenge_deadline_height)?;
    if simulation.funding_coin_id != report.funding_coin_id
        || simulation.closing_coin_kind != ClosingCoinKind::Initial
        || simulation.current_state_sequence != 0
        || simulation.latest_state_sequence != 1
        || simulation.initial_birth_height != initial_birth_height
        || simulation.challenge_deadline_height != challenge_deadline_height
        || simulation.assert_my_birth_height != Some(initial_birth_height)
        || simulation.assert_before_height_absolute != challenge_deadline_height
        || simulation.closing_amount_mojo != 5
        || !simulation.recovery_packages_verified
        || !simulation.all_clvm_conditions_verified
        || simulation.spend_bundle_created
        || simulation.broadcast_ready
        || simulation.chain_broadcast
    {
        return Err("state 0 challenge simulation did not preserve safety bindings".into());
    }

    Ok(StateZeroChallengeReport {
        schema: OUTPUT_SCHEMA,
        release_status: "LOCAL_CHALLENGE_REHEARSAL_ONLY",
        protocol_version: "0x0360",
        network: "mainnet",
        mainnet_approved: false,
        test_only: true,
        recovery_package_content_hash: report.recovery_package_content_hash,
        spend_bundle_created: false,
        broadcast_ready: false,
        chain_broadcast: false,
        simulation,
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
    fn simulates_the_bound_state_zero_to_state_one_challenge() {
        let output = build_report(report(), 9_159_459, 9_165_459).expect("challenge simulation");
        assert_eq!(
            output.simulation.closing_coin_kind,
            ClosingCoinKind::Initial
        );
        assert_eq!(
            (
                output.simulation.current_state_sequence,
                output.simulation.latest_state_sequence,
            ),
            (0, 1)
        );
        assert_eq!(output.simulation.assert_my_birth_height, Some(9_159_459));
        assert_eq!(output.simulation.assert_before_height_absolute, 9_165_459);
        assert!(output.simulation.all_clvm_conditions_verified);
    }

    #[test]
    fn rejects_a_deadline_not_bound_to_challenge_blocks() {
        let error = build_report(report(), 9_159_459, 9_165_458).unwrap_err();
        assert!(error.contains("challenge deadline"));
    }

    #[test]
    fn rejects_a_report_that_does_not_bind_state_one() {
        let mut input = report();
        input.state_sequence = 0;
        assert!(build_report(input, 9_159_459, 9_165_459).is_err());
    }

    #[test]
    fn output_cannot_enable_a_spend_or_broadcast() {
        let output = build_report(report(), 9_159_459, 9_165_459).expect("challenge simulation");
        assert!(output.test_only);
        assert!(!output.mainnet_approved);
        assert!(!output.spend_bundle_created);
        assert!(!output.broadcast_ready);
        assert!(!output.chain_broadcast);
        assert!(!output.simulation.spend_bundle_created);
        assert!(!output.simulation.broadcast_ready);
        assert!(!output.simulation.chain_broadcast);
    }
}
