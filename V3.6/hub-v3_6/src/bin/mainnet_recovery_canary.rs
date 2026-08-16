use std::{env, fs, path::PathBuf, time::SystemTime};

use chia_bls::SecretKey;
use serde::{Deserialize, Serialize};
use xhub_hub_v3_6::{
    ChainChannelRegistration, ChiaFullNodeRpcConfig, ChiaFullNodeRpcProvider, HubStore,
    ReservationRequest,
};
use xhub_protocol_v3_6::{
    CanonicalDecode, CanonicalEncode, ChannelTerms, LedgerEntry, ReservationStatus,
};

const MAINNET_NETWORK_ID: &str = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";
const FUNDING_CONFIRMATIONS: u64 = 32;

#[derive(Debug, Deserialize)]
struct FundingCandidate {
    protocol_version: String,
    network: String,
    network_id: String,
    funding_amount_mojo: u64,
    channel_terms_hash: String,
    channel_terms_canonical_hex: String,
    funding_puzzle_hash: String,
    funding_puzzle_reveal: String,
    mainnet_approved: bool,
    broadcast_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HubReservationRequest {
    protocol_version: String,
    request_id: String,
    funding_coin_id: String,
    merchant_puzzle_hash: String,
    merchant_receipt_public_key: String,
    amount: String,
    reservation_nonce: String,
    user_authorization_signature: String,
}

#[derive(Debug, Serialize)]
struct RecoveryPackageReport {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    funding_coin_id: String,
    funding_puzzle_hash: String,
    funding_amount_mojo: u64,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    recovery_package_canonical_hex: String,
    hub_status: &'static str,
    ledger_written: bool,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    chain_broadcast: bool,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 6 {
        return Err(
            "usage: mainnet-recovery-canary FUNDING_CANDIDATE HUB_REQUEST RPC_URL HUB_DB HUB_BLS_SECRET OUTPUT_JSON"
                .into(),
        );
    }
    let candidate: FundingCandidate = read_json(&args[0], "funding candidate")?;
    let hub_request: HubReservationRequest = read_json(&args[1], "HUB reservation request")?;
    let terms = validate_candidate(&candidate)?;
    let funding_coin_id = fixed_hex::<32>(&hub_request.funding_coin_id, "funding coin ID")?;
    let registration = ChainChannelRegistration {
        funding_coin_id,
        funding_puzzle_reveal: decode_hex(
            &candidate.funding_puzzle_reveal,
            "funding puzzle reveal",
        )?,
        channel_terms: terms.clone(),
    };
    let request = reservation_request(&hub_request)?;
    if request.funding_coin_id != funding_coin_id {
        return Err("HUB request Funding Coin differs from registration".into());
    }

    let rpc = ChiaFullNodeRpcProvider::connect(ChiaFullNodeRpcConfig::public(&args[2]))
        .map_err(|error| error.to_string())?;
    let db_path = PathBuf::from(&args[3]);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut store = HubStore::open(&db_path).map_err(|error| error.to_string())?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let snapshot = store
        .register_channel_from_chain(&registration, &rpc, FUNDING_CONFIRMATIONS, now)
        .map_err(|error| error.to_string())?;
    if snapshot.funding_coin_id != funding_coin_id {
        return Err("registered channel differs from Funding Coin".into());
    }

    let hub_secret = load_secret(&PathBuf::from(&args[4]))?;
    let outcome = store
        .reserve_with_chain(&request, &rpc, &hub_secret, now)
        .map_err(|error| error.to_string())?;
    let result = &outcome.signed_result.result;
    if result.status != ReservationStatus::Signed || !result.ledger_written {
        return Err(format!(
            "HUB reservation was not signed: {:?}",
            result.status
        ));
    }
    outcome
        .signed_result
        .verify(&terms)
        .map_err(|error| format!("invalid HUB result signature: {error}"))?;
    let package = outcome
        .recovery_package
        .ok_or("signed reservation did not create a RecoveryPackage")?;
    package
        .validate()
        .map_err(|error| format!("invalid RecoveryPackage: {error}"))?;
    let state_sequence = result.state_sequence.ok_or("missing state sequence")?;
    let checkpoint_hash = result.checkpoint_hash.ok_or("missing checkpoint hash")?;
    if package.official_state.checkpoint.state_sequence != state_sequence
        || package
            .official_state
            .checkpoint
            .hash(&terms)
            .map_err(|error| error.to_string())?
            != checkpoint_hash
    {
        return Err("RecoveryPackage differs from the signed reservation result".into());
    }

    let report = RecoveryPackageReport {
        schema: "xhub-v3-6-mainnet-recovery-package-1",
        protocol_version: "0x0360",
        network: "mainnet",
        funding_coin_id: hex::encode(funding_coin_id),
        funding_puzzle_hash: candidate.funding_puzzle_hash,
        funding_amount_mojo: package.funding_amount,
        state_sequence,
        checkpoint_hash: hex::encode(checkpoint_hash),
        recovery_package_content_hash: hex::encode(
            package.content_hash().map_err(|error| error.to_string())?,
        ),
        recovery_package_canonical_hex: hex::encode(package.canonical_bytes()),
        hub_status: "SIGNED",
        ledger_written: true,
        spend_bundle_created: false,
        broadcast_enabled: false,
        chain_broadcast: false,
    };
    write_json(PathBuf::from(&args[5]), &report)?;
    println!("status=RECOVERY_PACKAGE_GENERATED");
    println!("state_sequence={state_sequence}");
    println!(
        "recovery_package_content_hash={}",
        report.recovery_package_content_hash
    );
    println!("output={}", args[5]);
    Ok(())
}

fn validate_candidate(candidate: &FundingCandidate) -> Result<ChannelTerms, String> {
    if candidate.protocol_version != "0x0360"
        || candidate.network != "mainnet"
        || candidate.network_id != MAINNET_NETWORK_ID
        || candidate.funding_amount_mojo != 5
        || candidate.mainnet_approved
        || candidate.broadcast_enabled
    {
        return Err("candidate is not the non-broadcast 5-mojo mainnet recovery canary".into());
    }
    let terms = ChannelTerms::from_canonical_bytes(&decode_hex(
        &candidate.channel_terms_canonical_hex,
        "channel terms",
    )?)
    .map_err(|error| error.to_string())?;
    if terms.network_id != fixed_hex::<32>(MAINNET_NETWORK_ID, "network ID")?
        || terms.funding_amount != 5
        || hex::encode(terms.hash().map_err(|error| error.to_string())?)
            != candidate.channel_terms_hash
    {
        return Err("candidate channel terms binding is invalid".into());
    }
    Ok(terms)
}

fn reservation_request(value: &HubReservationRequest) -> Result<ReservationRequest, String> {
    if value.protocol_version != "0x0360" || value.amount != "1" {
        return Err("HUB request must be an exact 1-mojo V3.6 request".into());
    }
    let request = ReservationRequest {
        request_id: fixed_hex(&value.request_id, "request ID")?,
        funding_coin_id: fixed_hex(&value.funding_coin_id, "funding coin ID")?,
        ledger_entry: LedgerEntry {
            merchant_puzzle_hash: fixed_hex(&value.merchant_puzzle_hash, "merchant puzzle hash")?,
            merchant_receipt_public_key: fixed_hex(
                &value.merchant_receipt_public_key,
                "merchant receipt public key",
            )?,
            amount: 1,
            reservation_nonce: fixed_hex(&value.reservation_nonce, "reservation nonce")?,
        },
        user_authorization_signature: fixed_hex(
            &value.user_authorization_signature,
            "user authorization signature",
        )?,
    };
    request.fingerprint().map_err(|error| error.to_string())?;
    Ok(request)
}

fn load_secret(path: &PathBuf) -> Result<SecretKey, String> {
    let content =
        fs::read_to_string(path).map_err(|error| format!("cannot read HUB BLS secret: {error}"))?;
    let bytes = fixed_hex::<32>(content.trim(), "HUB BLS secret")?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str, name: &str) -> Result<T, String> {
    serde_json::from_str(
        &fs::read_to_string(path).map_err(|error| format!("cannot read {name}: {error}"))?,
    )
    .map_err(|error| format!("invalid {name}: {error}"))
}

fn fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    decode_hex(value, field)?
        .try_into()
        .map_err(|_| format!("{field} must be {N} bytes"))
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
