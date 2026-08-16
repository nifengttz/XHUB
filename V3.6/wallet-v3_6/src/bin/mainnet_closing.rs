use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage};
use xhub_puzzles_v3_6::{ClosingSimulation, simulate_recovery_closing};

const FUNDING_COIN_ID: &str = "d8d089881dde12de0bdb8a078df9ab047da307d3f671b5b188b78448d570ea9d";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageResponse {
    protocol_version: String,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    recovery_package_canonical_hex: String,
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
        return Err("usage: mainnet-closing PACKAGE_JSON START_CLOSE_HEIGHT OUTPUT_JSON".into());
    }
    let response: PackageResponse =
        serde_json::from_str(&fs::read_to_string(&args[0]).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid RecoveryPackage response JSON: {error}"))?;
    if response.protocol_version != "0x0360"
        || response.funding_coin_id != FUNDING_COIN_ID
        || response.state_sequence != 1
    {
        return Err("RecoveryPackage is not the expected V3.6 mainnet experiment state".into());
    }
    let bytes = hex::decode(
        response
            .recovery_package_canonical_hex
            .strip_prefix("0x")
            .unwrap_or(&response.recovery_package_canonical_hex),
    )
    .map_err(|error| format!("invalid RecoveryPackage canonical hex: {error}"))?;
    let package = RecoveryPackage::from_canonical_bytes(&bytes)
        .map_err(|error| format!("invalid RecoveryPackage: {error}"))?;
    let start_close_height = args[1]
        .parse::<u64>()
        .map_err(|_| "START_CLOSE_HEIGHT must be a u64")?;
    let simulation = simulate_recovery_closing(&package, start_close_height)?;
    if simulation.funding_coin_id != response.funding_coin_id
        || simulation.state_sequence != response.state_sequence
        || simulation.checkpoint_hash != response.checkpoint_hash
    {
        return Err("simulation does not match the RecoveryPackage response binding".into());
    }
    let report = ClosingReport {
        schema: "xhub-v3-6-mainnet-closing-simulation-1",
        release_status: "UNAUDITED_MAINNET_EXPERIMENT",
        mainnet_approved: false,
        network: "mainnet",
        chain_broadcast: false,
        spend_bundle_created: false,
        recovery_package_content_hash: response.recovery_package_content_hash,
        simulation,
    };
    write_json(&args[2], &report)?;
    println!("closing_simulation={}", args[2]);
    Ok(())
}

fn write_json(path: &str, value: &impl Serialize) -> Result<(), String> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())
}
