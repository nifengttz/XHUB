use std::io::{self, Read};

use bech32::{FromBase32, ToBase32, Variant};
use bip39::{Language, Mnemonic};
use chia_bls::{SecretKey, master_to_wallet_unhardened};
use chia_puzzle_types::{DeriveSynthetic, standard::StandardArgs};
use serde::{Deserialize, Serialize};

const ADDRESS_INDEX: u32 = 0;
const DERIVATION_PATH: &str = "m/12381/8444/2/0";

#[derive(Debug, Serialize)]
struct WalletMaterial {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    single_address_mode: bool,
    address_index: u32,
    derivation_path: &'static str,
    password_required: bool,
    storage_protection: &'static str,
    mnemonic: String,
    master_private_key: String,
    master_public_key: String,
    wallet_private_key_index0: String,
    wallet_public_key_index0: String,
    synthetic_private_key_index0: String,
    synthetic_public_key_index0: String,
    puzzle_hash: String,
    address: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewInput {
    source_coin_id: String,
    destination_address: String,
    amount_mojo: u64,
    fee_mojo: u64,
    purpose: String,
}

#[derive(Debug, Serialize)]
struct PreviewOutput {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    source_coin_id: String,
    destination_address: String,
    destination_puzzle_hash: String,
    amount_mojo: u64,
    fee_mojo: u64,
    total_mojo: u64,
    purpose: String,
    preview_only: bool,
    spend_bundle_created: bool,
    rpc_called: bool,
    push_tx_called: bool,
    chain_broadcast: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    match command.as_str() {
        "generate" => {
            let mnemonic = Mnemonic::generate_in(Language::English, 24)
                .map_err(|error| format!("failed to generate mnemonic: {error}"))?;
            print_wallet(&mnemonic)
        }
        "restore" => {
            let phrase = read_stdin()?;
            let normalized = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
            let mnemonic = Mnemonic::parse_in_normalized(Language::English, &normalized)
                .map_err(|error| format!("invalid 24-word English mnemonic: {error}"))?;
            if mnemonic.word_count() != 24 {
                return Err(format!(
                    "V3.6 wallet requires exactly 24 words; received {}",
                    mnemonic.word_count()
                ));
            }
            print_wallet(&mnemonic)
        }
        "preview" => preview(),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: xhub-wallet-core-v3-6 <generate|restore|preview>\nrestore and preview read their input from stdin; this binary never connects to a network"
        .to_string()
}

fn read_stdin() -> Result<String, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("failed to read stdin: {error}"))?;
    if input.trim().is_empty() {
        return Err("stdin input is empty".to_string());
    }
    Ok(input)
}

fn wallet_material(mnemonic: &Mnemonic) -> Result<WalletMaterial, String> {
    let seed = mnemonic.to_seed("");
    let master = SecretKey::from_seed(&seed);
    let wallet = master_to_wallet_unhardened(&master, ADDRESS_INDEX);
    let synthetic = wallet.derive_synthetic();
    let puzzle_hash = StandardArgs::curry_tree_hash(synthetic.public_key());
    let address = bech32::encode("xch", puzzle_hash.to_base32(), Variant::Bech32m)
        .map_err(|error| format!("address encoding error: {error}"))?;

    Ok(WalletMaterial {
        schema: "xhub.wallet.v3_6.material.v1",
        protocol_version: "3.6",
        network: "chia-mainnet",
        single_address_mode: true,
        address_index: ADDRESS_INDEX,
        derivation_path: DERIVATION_PATH,
        password_required: false,
        storage_protection: "none-plaintext-local-file",
        mnemonic: mnemonic.to_string(),
        master_private_key: hex::encode(master.to_bytes()),
        master_public_key: hex::encode(master.public_key().to_bytes()),
        wallet_private_key_index0: hex::encode(wallet.to_bytes()),
        wallet_public_key_index0: hex::encode(wallet.public_key().to_bytes()),
        synthetic_private_key_index0: hex::encode(synthetic.to_bytes()),
        synthetic_public_key_index0: hex::encode(synthetic.public_key().to_bytes()),
        puzzle_hash: hex::encode(puzzle_hash),
        address,
    })
}

fn print_wallet(mnemonic: &Mnemonic) -> Result<(), String> {
    let material = wallet_material(mnemonic)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&material)
            .map_err(|error| format!("JSON encoding error: {error}"))?
    );
    Ok(())
}

fn preview() -> Result<(), String> {
    let input: PreviewInput = serde_json::from_str(&read_stdin()?)
        .map_err(|error| format!("invalid preview JSON: {error}"))?;
    validate_hex32("source_coin_id", &input.source_coin_id)?;
    if input.amount_mojo == 0 {
        return Err("amount_mojo must be greater than zero".to_string());
    }
    if input.purpose.trim().is_empty() {
        return Err("purpose must not be empty".to_string());
    }
    let total_mojo = input
        .amount_mojo
        .checked_add(input.fee_mojo)
        .ok_or_else(|| "amount_mojo + fee_mojo overflow".to_string())?;
    let (prefix, data, variant) = bech32::decode(&input.destination_address)
        .map_err(|error| format!("invalid destination_address: {error}"))?;
    let puzzle_hash = Vec::<u8>::from_base32(&data)
        .map_err(|error| format!("invalid destination puzzle hash: {error}"))?;
    if prefix != "xch" || variant != Variant::Bech32m || puzzle_hash.len() != 32 {
        return Err("destination_address must be a Chia mainnet xch Bech32m address".to_string());
    }

    let output = PreviewOutput {
        schema: "xhub.wallet.v3_6.transaction_preview.v1",
        protocol_version: "3.6",
        network: "chia-mainnet",
        source_coin_id: input.source_coin_id.to_lowercase(),
        destination_address: input.destination_address,
        destination_puzzle_hash: hex::encode(puzzle_hash),
        amount_mojo: input.amount_mojo,
        fee_mojo: input.fee_mojo,
        total_mojo,
        purpose: input.purpose.trim().to_string(),
        preview_only: true,
        spend_bundle_created: false,
        rpc_called: false,
        push_tx_called: false,
        chain_broadcast: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("JSON encoding error: {error}"))?
    );
    Ok(())
}

fn validate_hex32(name: &str, value: &str) -> Result<(), String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let decoded = hex::decode(value).map_err(|_| format!("{name} must be 32-byte hex"))?;
    if decoded.len() != 32 {
        return Err(format!("{name} must be 32-byte hex"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn derives_only_index_zero_deterministically() {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, TEST_MNEMONIC).unwrap();
        let first = wallet_material(&mnemonic).unwrap();
        let second = wallet_material(&mnemonic).unwrap();
        assert_eq!(first.address_index, 0);
        assert_eq!(first.derivation_path, "m/12381/8444/2/0");
        assert!(first.single_address_mode);
        assert!(!first.password_required);
        assert_eq!(first.address, second.address);
        assert!(first.address.starts_with("xch1"));
        assert_eq!(first.puzzle_hash.len(), 64);
    }

    #[test]
    fn rejects_non_32_byte_coin_id() {
        assert!(validate_hex32("source_coin_id", "abcd").is_err());
    }
}
