use std::{env, fs, path::PathBuf, time::SystemTime};

use chia_bls::SecretKey;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage, public_key_bytes};
use xhub_watchtower_v3_6::WatchtowerStore;

#[derive(Debug, Deserialize)]
struct RecoveryPackageReport {
    schema: String,
    protocol_version: String,
    network: String,
    funding_coin_id: String,
    funding_amount_mojo: u64,
    state_sequence: u64,
    recovery_package_content_hash: String,
    recovery_package_canonical_hex: String,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct WatchtowerReport {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    funding_coin_id: String,
    funding_amount_mojo: u64,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    entry_count: u64,
    package_status: &'static str,
    confirmation_status: &'static str,
    local_greenlight_delivered: bool,
    local_signer_count: u16,
    local_failure_domain_count: u16,
    production_threshold: u16,
    production_greenlight_delivered: bool,
    production_signer_count: u16,
    production_failure_domain_count: u16,
    production_threshold_met: bool,
    durability_journal_mode: String,
    durability_synchronous: i64,
    quarantine_count: usize,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    chain_broadcast: bool,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: watchtower-mainnet-recovery-canary RECOVERY_PACKAGE WATCHTOWER_DB MERCHANT_RECEIPT_SECRET OUTPUT_JSON"
                .into(),
        );
    }
    let report: RecoveryPackageReport = read_json(&args[0], "RecoveryPackage report")?;
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
    let package_bytes = decode_hex(
        &report.recovery_package_canonical_hex,
        "RecoveryPackage canonical bytes",
    )?;
    let package =
        RecoveryPackage::from_canonical_bytes(&package_bytes).map_err(|error| error.to_string())?;
    package.validate().map_err(|error| error.to_string())?;
    if hex::encode(package.funding_coin_id) != report.funding_coin_id
        || package.funding_amount != 5
        || package.official_state.checkpoint.state_sequence != 1
        || hex::encode(package.content_hash().map_err(|error| error.to_string())?)
            != report.recovery_package_content_hash
    {
        return Err("RecoveryPackage report binding is invalid".into());
    }

    let db_path = PathBuf::from(&args[1]);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let mut store = WatchtowerStore::open(&db_path).map_err(|error| error.to_string())?;
    let accepted = store
        .accept_package(&package_bytes, now)
        .map_err(|error| error.to_string())?;
    let merchant_secret = load_secret(&PathBuf::from(&args[2]))?;
    let signer_id = "merchant-mainnet-recovery-canary-005";
    let failure_domain = "local-mainnet-recovery-canary";
    store
        .register_confirmer(
            signer_id,
            failure_domain,
            public_key_bytes(&merchant_secret),
            now,
        )
        .map_err(|error| error.to_string())?;
    let signed = store
        .sign_confirmation(
            package.funding_coin_id,
            accepted.state_sequence,
            0,
            signer_id,
            &merchant_secret,
        )
        .map_err(|error| error.to_string())?;
    store
        .record_confirmation(&signed, now)
        .map_err(|error| error.to_string())?;
    let local = store
        .greenlight_status(package.funding_coin_id, accepted.state_sequence, 0, 1)
        .map_err(|error| error.to_string())?;
    let production = store
        .greenlight_status(package.funding_coin_id, accepted.state_sequence, 0, 2)
        .map_err(|error| error.to_string())?;
    if !local.delivered || production.delivered {
        return Err("unexpected local or production greenlight status".into());
    }
    let durability = store.durability_mode().map_err(|error| error.to_string())?;
    let quarantine_count = store
        .quarantined()
        .map_err(|error| error.to_string())?
        .len();
    let output = WatchtowerReport {
        schema: "xhub-v3-6-mainnet-watchtower-ingest-1",
        protocol_version: "0x0360",
        network: "mainnet",
        funding_coin_id: report.funding_coin_id,
        funding_amount_mojo: report.funding_amount_mojo,
        state_sequence: accepted.state_sequence,
        checkpoint_hash: hex::encode(accepted.checkpoint_hash),
        recovery_package_content_hash: hex::encode(accepted.recovery_package_content_hash),
        entry_count: accepted.entry_count,
        package_status: "ACCEPTED",
        confirmation_status: "ACCEPTED",
        local_greenlight_delivered: local.delivered,
        local_signer_count: local.signer_count,
        local_failure_domain_count: local.failure_domain_count,
        production_threshold: 2,
        production_greenlight_delivered: production.delivered,
        production_signer_count: production.signer_count,
        production_failure_domain_count: production.failure_domain_count,
        production_threshold_met: false,
        durability_journal_mode: durability.0,
        durability_synchronous: durability.1,
        quarantine_count,
        spend_bundle_created: false,
        broadcast_enabled: false,
        chain_broadcast: false,
    };
    write_json(PathBuf::from(&args[3]), &output)?;
    println!("status=WATCHTOWER_PACKAGE_ACCEPTED");
    println!("local_greenlight=1-of-1");
    println!("production_greenlight=BLOCKED_1_OF_2");
    println!("output={}", args[3]);
    Ok(())
}

fn load_secret(path: &PathBuf) -> Result<SecretKey, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read merchant receipt secret: {error}"))?;
    let bytes: [u8; 32] = decode_hex(content.trim(), "merchant receipt secret")?
        .try_into()
        .map_err(|_| "merchant receipt secret must be 32 bytes")?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str, name: &str) -> Result<T, String> {
    serde_json::from_str(
        &fs::read_to_string(path).map_err(|error| format!("cannot read {name}: {error}"))?,
    )
    .map_err(|error| format!("invalid {name}: {error}"))
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
