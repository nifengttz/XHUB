use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage};
use xhub_puzzles_v3_6::{ClosingSimulation, simulate_recovery_closing};

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
struct ClosingReport {
    schema: &'static str,
    release_status: &'static str,
    mainnet_approved: bool,
    network: &'static str,
    chain_broadcast: bool,
    spend_bundle_created: bool,
    recovery_package_content_hash: String,
    simulation: ClosingSimulation,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(
            "usage: mainnet-recovery-canary-closing RECOVERY_PACKAGE START_CLOSE_HEIGHT OUTPUT_JSON"
                .into(),
        );
    }
    let report: RecoveryPackageReport =
        serde_json::from_str(&fs::read_to_string(&args[0]).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid RecoveryPackage report: {error}"))?;
    if report.schema != "xhub-v3-6-mainnet-recovery-package-1"
        || report.protocol_version != "0x0360"
        || report.network != "mainnet"
        || report.funding_amount_mojo != 5
        || report.state_sequence != 1
        || report.spend_bundle_created
        || report.broadcast_enabled
        || report.chain_broadcast
    {
        return Err("input is not the non-broadcast 5-mojo RecoveryPackage".into());
    }
    let bytes = hex::decode(
        report
            .recovery_package_canonical_hex
            .strip_prefix("0x")
            .unwrap_or(&report.recovery_package_canonical_hex),
    )
    .map_err(|error| format!("invalid RecoveryPackage bytes: {error}"))?;
    let package = RecoveryPackage::from_canonical_bytes(&bytes)
        .map_err(|error| format!("invalid RecoveryPackage: {error}"))?;
    package.validate().map_err(|error| error.to_string())?;
    if hex::encode(package.funding_coin_id) != report.funding_coin_id
        || package.funding_amount != report.funding_amount_mojo
        || package.official_state.checkpoint.state_sequence != report.state_sequence
        || hex::encode(
            package
                .official_state
                .checkpoint
                .hash(&package.channel_terms)
                .map_err(|error| error.to_string())?,
        ) != report.checkpoint_hash
        || hex::encode(package.content_hash().map_err(|error| error.to_string())?)
            != report.recovery_package_content_hash
    {
        return Err("RecoveryPackage report binding is invalid".into());
    }
    let start_close_height = args[1]
        .parse::<u64>()
        .map_err(|_| "START_CLOSE_HEIGHT must be a u64")?;
    let simulation = simulate_recovery_closing(&package, start_close_height)?;
    if simulation.funding_coin_id != report.funding_coin_id
        || simulation.funding_amount_mojo != 5
        || simulation.state_sequence != 1
        || simulation.checkpoint_hash != report.checkpoint_hash
        || !simulation.recovery_package_verified
        || !simulation.all_clvm_conditions_verified
        || simulation.broadcast_ready
        || simulation.chain_broadcast
    {
        return Err("Closing simulation did not preserve recovery canary bindings".into());
    }
    let output = ClosingReport {
        schema: "xhub-v3-6-mainnet-closing-simulation-1",
        release_status: "RECOVERY_CANARY_LOCAL_ONLY",
        mainnet_approved: false,
        network: "mainnet",
        chain_broadcast: false,
        spend_bundle_created: false,
        recovery_package_content_hash: report.recovery_package_content_hash,
        simulation,
    };
    write_json(PathBuf::from(&args[2]), &output)?;
    println!("status=RECOVERY_CLOSING_SIMULATED");
    println!("output={}", args[2]);
    Ok(())
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())
}
