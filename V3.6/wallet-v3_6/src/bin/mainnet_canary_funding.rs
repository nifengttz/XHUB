use std::{env, fs, path::PathBuf};

use bech32::{FromBase32, ToBase32, Variant};
use chia_bls::SecretKey;
use serde::Serialize;
use xhub_protocol_v3_6::{parse_public_key, public_key_bytes, state_rules_hash};
use xhub_wallet_v3_6::{FundingTermsInput, preview};

const MAINNET_GENESIS: &str = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";

#[derive(Serialize)]
struct FundingCandidate {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    network_id: &'static str,
    request_id: String,
    funding_amount_mojo: u64,
    fee_mojo: u64,
    acceptance_blocks: u64,
    freeze_blocks: u64,
    close_delay_blocks: u64,
    challenge_blocks: u64,
    user_public_key: String,
    user_remainder_puzzle_hash: String,
    hub_state_public_key_a: String,
    channel_terms_hash: String,
    channel_terms_canonical_hex: String,
    funding_puzzle_hash: String,
    funding_address: String,
    funding_puzzle_reveal: String,
    mainnet_approved: bool,
    broadcast_enabled: bool,
    wallet_confirmation_required: bool,
}

#[derive(Serialize)]
struct HubwalletLockRequest {
    request_id: String,
    network: &'static str,
    target_address: String,
    amount_mojo: u64,
    memo: &'static str,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 && args.len() != 6 {
        return Err("usage: mainnet-canary-funding USER_PUBLIC_KEY USER_PUZZLE_HASH REQUEST_ID OUTPUT_DIRECTORY HUB_BLS_IDENTITY_FILE [FUNDING_AMOUNT_MOJO]".into());
    }
    let user_public_key = fixed_hex(&args[0], 48, "user public key")?;
    let user_puzzle_hash = fixed_hex(&args[1], 32, "user puzzle hash")?;
    if args[2].is_empty() || args[2].len() > 128 {
        return Err("request ID must be 1-128 characters".into());
    }
    let output_directory = PathBuf::from(&args[3]);
    let hub_public_key = load_hub_public_key(&PathBuf::from(&args[4]))?;
    let funding_amount = match args.get(5) {
        Some(value) => parse_funding_amount(value)?,
        None => 1,
    };
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
        hub_state_public_key_a: hub_public_key,
        state_rules_hash: hex::encode(rules),
        funding_amount: funding_amount.to_string(),
        user_remainder_puzzle_hash: user_puzzle_hash.clone(),
    };
    let terms = input
        .to_channel_terms()
        .map_err(|error| error.to_string())?;
    let result = preview(&terms).map_err(|error| error.to_string())?;
    let puzzle_hash = hex::decode(&result.funding_puzzle_hash).map_err(|e| e.to_string())?;
    let address = bech32::encode("xch", puzzle_hash.to_base32(), Variant::Bech32m)
        .map_err(|error| error.to_string())?;
    let (prefix, decoded, variant) = bech32::decode(&address).map_err(|e| e.to_string())?;
    let decoded = Vec::<u8>::from_base32(&decoded).map_err(|e| e.to_string())?;
    if prefix != "xch" || variant != Variant::Bech32m || decoded != puzzle_hash {
        return Err("Funding address failed Bech32m verification".into());
    }
    let candidate = FundingCandidate {
        schema: "xhub-v3-6-mainnet-canary-funding-1",
        protocol_version: "0x0360",
        network: "mainnet",
        network_id: MAINNET_GENESIS,
        request_id: args[2].clone(),
        funding_amount_mojo: funding_amount,
        fee_mojo: 0,
        acceptance_blocks: terms.acceptance_blocks,
        freeze_blocks: terms.freeze_blocks,
        close_delay_blocks: terms.close_delay_blocks,
        challenge_blocks: terms.challenge_blocks,
        user_public_key,
        user_remainder_puzzle_hash: user_puzzle_hash,
        hub_state_public_key_a: input.hub_state_public_key_a,
        channel_terms_hash: result.channel_terms_hash,
        channel_terms_canonical_hex: result.channel_terms_canonical_hex,
        funding_puzzle_hash: result.funding_puzzle_hash,
        funding_address: address.clone(),
        funding_puzzle_reveal: result.funding_puzzle_reveal,
        mainnet_approved: false,
        broadcast_enabled: false,
        wallet_confirmation_required: true,
    };
    let request = HubwalletLockRequest {
        request_id: args[2].clone(),
        network: "mainnet",
        target_address: address,
        amount_mojo: funding_amount,
        memo: "xhub-v3.6-mainnet-canary-funding",
    };
    fs::create_dir_all(&output_directory).map_err(|e| e.to_string())?;
    write_json(output_directory.join("funding-candidate.json"), &candidate)?;
    write_json(
        output_directory.join("hubwallet-lock-request.json"),
        &request,
    )?;
    println!(
        "funding_candidate={}",
        output_directory.join("funding-candidate.json").display()
    );
    println!(
        "hubwallet_request={}",
        output_directory
            .join("hubwallet-lock-request.json")
            .display()
    );
    println!("funding_address={}", candidate.funding_address);
    Ok(())
}

fn parse_funding_amount(value: &str) -> Result<u64, String> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value.bytes().any(|byte| !byte.is_ascii_digit())
    {
        return Err("funding amount must be a canonical unsigned mojo integer".into());
    }
    let amount = value
        .parse::<u64>()
        .map_err(|_| "funding amount is out of range".to_string())?;
    if amount == 0 {
        return Err("funding amount must be positive".into());
    }
    Ok(amount)
}

fn fixed_hex(value: &str, length: usize, name: &str) -> Result<String, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|_| format!("{name} must be hexadecimal"))?;
    if bytes.len() != length {
        return Err(format!("{name} must be {length} bytes"));
    }
    Ok(hex::encode(bytes))
}

fn load_hub_public_key(path: &PathBuf) -> Result<String, String> {
    let value = fs::read_to_string(path)
        .map_err(|error| format!("cannot read HUB BLS identity: {error}"))?;
    hub_public_key_from_hex(value.trim())
}

fn hub_public_key_from_hex(value: &str) -> Result<String, String> {
    let bytes = hex::decode(value).map_err(|_| "HUB BLS identity must be hexadecimal")?;
    if let Ok(public_key) = <[u8; 48]>::try_from(bytes.as_slice()) {
        parse_public_key(&public_key).map_err(|error| error.to_string())?;
        return Ok(hex::encode(public_key));
    }
    if bytes.len() == 32 {
        let secret = load_secret_bytes(&bytes)?;
        return Ok(hex::encode(public_key_bytes(&secret)));
    }
    Err("HUB BLS identity must be a 48-byte public key or legacy 32-byte secret key".into())
}

fn load_secret_bytes(bytes: &[u8]) -> Result<SecretKey, String> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "HUB BLS secret must be 32 bytes")?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_identity_without_hub_secret() {
        let secret = SecretKey::from_seed(&[0x36; 32]);
        let expected = hex::encode(public_key_bytes(&secret));
        assert_eq!(hub_public_key_from_hex(&expected), Ok(expected));
    }

    #[test]
    fn keeps_legacy_secret_identity_compatible() {
        let secret = SecretKey::from_seed(&[0x37; 32]);
        assert_eq!(
            hub_public_key_from_hex(&hex::encode(secret.to_bytes())),
            Ok(hex::encode(public_key_bytes(&secret)))
        );
    }
}
