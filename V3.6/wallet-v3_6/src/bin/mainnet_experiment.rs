use std::{env, fs, path::PathBuf};

use bech32::{FromBase32, ToBase32, Variant};
use chia_bls::SecretKey;
use rand::RngCore;
use serde::Serialize;
use xhub_protocol_v3_6::{public_key_bytes, state_rules_hash};
use xhub_wallet_v3_6::{FundingTermsInput, preview};

const MAINNET_GENESIS: &str = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";

#[derive(Serialize)]
struct Experiment<'a> {
    schema: &'a str,
    protocol_version: &'a str,
    release_status: &'a str,
    mainnet_approved: bool,
    rpc_url: &'a str,
    network: &'a str,
    network_id: &'a str,
    wallet_fingerprint: u32,
    wallet_request_id: &'a str,
    funding_amount_mojo: u64,
    fee_mojo: u64,
    user_public_key: &'a str,
    user_remainder_puzzle_hash: &'a str,
    hub_state_public_key_a: String,
    channel_terms_hash: String,
    channel_terms_canonical_hex: String,
    funding_puzzle_hash: String,
    funding_address: String,
    funding_puzzle_reveal: String,
    module_hashes: ModuleHashJson,
    broadcast_requires_wallet_confirmation: bool,
    warnings: [&'a str; 3],
}

#[derive(Serialize)]
struct ModuleHashJson {
    funding: String,
    initial_closing: String,
    subsequent_closing: String,
    merchant_payment: String,
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_public_key = args.next().ok_or("usage: mainnet-experiment USER_PUBLIC_KEY USER_PUZZLE_HASH REQUEST_ID [OUTPUT_JSON] [SECRET_FILE]")?;
    let user_puzzle_hash = args.next().ok_or("missing USER_PUZZLE_HASH")?;
    let request_id = args.next().ok_or("missing REQUEST_ID")?;
    let output = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "../mainnet-experiment/funding-10-mojo.json".into()),
    );
    let secret_path = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "../local-secrets/mainnet-experiment-hub-bls.hex".into()),
    );

    let hub_secret = load_or_generate_secret(&secret_path)?;
    let modules = xhub_puzzles_v3_6::module_hashes();
    let rules = state_rules_hash(
        &modules.initial_closing,
        &modules.subsequent_closing,
        &modules.merchant_payment,
    );
    let input = FundingTermsInput {
        network_id: MAINNET_GENESIS.into(),
        acceptance_blocks: "12288".into(),
        freeze_blocks: "200".into(),
        challenge_blocks: "6000".into(),
        user_public_key: user_public_key.clone(),
        hub_state_public_key_a: hex::encode(public_key_bytes(&hub_secret)),
        state_rules_hash: hex::encode(rules),
        funding_amount: "10".into(),
        user_remainder_puzzle_hash: user_puzzle_hash.clone(),
    };
    let terms = input
        .to_channel_terms()
        .map_err(|error| error.to_string())?;
    let result = preview(&terms).map_err(|error| error.to_string())?;
    let funding_puzzle_hash_bytes =
        hex::decode(&result.funding_puzzle_hash).map_err(|error| error.to_string())?;
    let funding_address = bech32::encode(
        "xch",
        funding_puzzle_hash_bytes.to_base32(),
        Variant::Bech32m,
    )
    .map_err(|error| error.to_string())?;
    let (prefix, decoded, variant) =
        bech32::decode(&funding_address).map_err(|error| error.to_string())?;
    let decoded = Vec::<u8>::from_base32(&decoded).map_err(|error| error.to_string())?;
    if prefix != "xch" || variant != Variant::Bech32m || decoded != funding_puzzle_hash_bytes {
        return Err("Funding address failed Bech32m round-trip verification".into());
    }
    let experiment = Experiment {
        schema: "xhub-v3-6-mainnet-experiment-1",
        protocol_version: "0x0360",
        release_status: "UNAUDITED_MAINNET_EXPERIMENT",
        mainnet_approved: false,
        rpc_url: "https://api.coinset.org",
        network: "mainnet",
        network_id: MAINNET_GENESIS,
        wallet_fingerprint: 1_648_103_239,
        wallet_request_id: &request_id,
        funding_amount_mojo: 10,
        fee_mojo: 0,
        user_public_key: &user_public_key,
        user_remainder_puzzle_hash: &user_puzzle_hash,
        hub_state_public_key_a: input.hub_state_public_key_a,
        channel_terms_hash: result.channel_terms_hash,
        channel_terms_canonical_hex: result.channel_terms_canonical_hex,
        funding_puzzle_hash: result.funding_puzzle_hash,
        funding_address,
        funding_puzzle_reveal: result.funding_puzzle_reveal,
        module_hashes: ModuleHashJson {
            funding: result.funding_module_hash,
            initial_closing: result.initial_closing_module_hash,
            subsequent_closing: result.subsequent_closing_module_hash,
            merchant_payment: result.merchant_payment_module_hash,
        },
        broadcast_requires_wallet_confirmation: true,
        warnings: [
            "This is a 10 mojo mainnet experiment, not an approved V3.6 release.",
            "The Funding Coin may be unrecoverable if the unaudited puzzle or off-chain flow is defective.",
            "Do not broadcast unless HUBWALLET shows exactly 10 mojo and zero fee to this funding puzzle hash.",
        ],
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(&experiment).map_err(|error| error.to_string())?;
    fs::write(&output, format!("{json}\n")).map_err(|error| error.to_string())?;
    println!("{}", output.display());
    println!("funding_puzzle_hash={}", experiment.funding_puzzle_hash);
    println!("funding_address={}", experiment.funding_address);
    Ok(())
}

fn load_or_generate_secret(path: &PathBuf) -> Result<SecretKey, String> {
    if path.exists() {
        let value = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let bytes: [u8; 32] = hex::decode(value.trim())
            .map_err(|error| error.to_string())?
            .try_into()
            .map_err(|_| "existing Hub BLS secret must be 32 bytes")?;
        return SecretKey::from_bytes(&bytes).map_err(|error| error.to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut seed = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let secret = SecretKey::from_seed(&seed);
    fs::write(path, format!("{}\n", hex::encode(secret.to_bytes())))
        .map_err(|error| error.to_string())?;
    Ok(secret)
}
