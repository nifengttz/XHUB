use std::{env, fs, path::PathBuf};

use chia_bls::SecretKey;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{CanonicalDecode, ChannelTerms, LedgerEntry, public_key_bytes};

const MAINNET_NETWORK_ID: &str = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";

#[derive(Debug, Deserialize)]
struct FundingCandidate {
    protocol_version: String,
    network: String,
    network_id: String,
    channel_terms_hash: String,
    channel_terms_canonical_hex: String,
    user_public_key: String,
    user_remainder_puzzle_hash: String,
    funding_amount_mojo: u64,
    mainnet_approved: bool,
    broadcast_enabled: bool,
}

#[derive(Debug, Serialize)]
struct PaymentRequest {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    network_id: &'static str,
    channel_terms_canonical_hex: String,
    channel_terms_hash: String,
    wallet_request_id: String,
    request_id: String,
    funding_coin_id: String,
    merchant_puzzle_hash: String,
    merchant_receipt_public_key: String,
    amount: String,
    reservation_nonce: String,
    authorization_hash: String,
    user_public_key: String,
    user_authorization_signature: Option<String>,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: mainnet-recovery-canary-payment FUNDING_CANDIDATE FUNDING_COIN_ID WALLET_CONNECT_REQUEST_ID MERCHANT_RECEIPT_SECRET OUTPUT_JSON"
                .into(),
        );
    }

    let candidate: FundingCandidate = serde_json::from_str(
        &fs::read_to_string(&args[0])
            .map_err(|error| format!("cannot read funding candidate: {error}"))?,
    )
    .map_err(|error| format!("invalid funding candidate: {error}"))?;
    validate_wallet_request_id(&args[2])?;
    let funding_coin_id = fixed_hex::<32>(&args[1], "funding coin ID")?;
    let merchant_secret = load_secret(&PathBuf::from(&args[3]))?;
    let request = prepare_request(&candidate, funding_coin_id, &args[2], &merchant_secret)?;
    write_json(PathBuf::from(&args[4]), &request)?;

    println!("payment_request={}", args[4]);
    println!("authorization_hash={}", request.authorization_hash);
    Ok(())
}

fn prepare_request(
    candidate: &FundingCandidate,
    funding_coin_id: [u8; 32],
    wallet_request_id: &str,
    merchant_secret: &SecretKey,
) -> Result<PaymentRequest, String> {
    if candidate.protocol_version != "0x0360"
        || candidate.network != "mainnet"
        || candidate.network_id != MAINNET_NETWORK_ID
        || candidate.funding_amount_mojo != 5
        || candidate.mainnet_approved
        || candidate.broadcast_enabled
    {
        return Err(
            "funding candidate is not the non-broadcast 5-mojo V3.6 recovery canary".into(),
        );
    }
    let terms = ChannelTerms::from_canonical_bytes(&decode_hex(
        &candidate.channel_terms_canonical_hex,
        "channel terms",
    )?)
    .map_err(|error| format!("invalid channel terms: {error}"))?;
    if terms.network_id != fixed_hex::<32>(MAINNET_NETWORK_ID, "mainnet network ID")?
        || terms.funding_amount != 5
        || hex::encode(terms.hash().map_err(|error| error.to_string())?)
            != candidate.channel_terms_hash
        || hex::encode(terms.user_public_key) != candidate.user_public_key
        || hex::encode(terms.user_remainder_puzzle_hash) != candidate.user_remainder_puzzle_hash
    {
        return Err("funding candidate does not match its canonical V3.6 channel terms".into());
    }

    let mut request_id = [0_u8; 32];
    let mut reservation_nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut request_id);
    rand::rngs::OsRng.fill_bytes(&mut reservation_nonce);
    let entry = LedgerEntry {
        // The recovery canary uses the user's own remainder puzzle as its only payee.
        merchant_puzzle_hash: terms.user_remainder_puzzle_hash,
        merchant_receipt_public_key: public_key_bytes(merchant_secret),
        amount: 1,
        reservation_nonce,
    };
    let authorization_hash = entry
        .authorization_hash(&terms, &funding_coin_id)
        .map_err(|error| error.to_string())?;

    Ok(PaymentRequest {
        schema: "xhub-v3-6-mainnet-payment-request-1",
        protocol_version: "0x0360",
        network: "mainnet",
        network_id: MAINNET_NETWORK_ID,
        channel_terms_canonical_hex: candidate.channel_terms_canonical_hex.clone(),
        channel_terms_hash: candidate.channel_terms_hash.clone(),
        wallet_request_id: wallet_request_id.into(),
        request_id: hex::encode(request_id),
        funding_coin_id: hex::encode(funding_coin_id),
        merchant_puzzle_hash: hex::encode(entry.merchant_puzzle_hash),
        merchant_receipt_public_key: hex::encode(entry.merchant_receipt_public_key),
        amount: entry.amount.to_string(),
        reservation_nonce: hex::encode(entry.reservation_nonce),
        authorization_hash: hex::encode(authorization_hash),
        user_public_key: hex::encode(terms.user_public_key),
        user_authorization_signature: None,
    })
}

fn load_secret(path: &PathBuf) -> Result<SecretKey, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read merchant receipt secret: {error}"))?;
    let bytes: [u8; 32] = decode_hex(content.trim(), "merchant receipt secret")?
        .try_into()
        .map_err(|_| "merchant receipt secret must be 32 bytes")?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    decode_hex(value, field)?
        .try_into()
        .map_err(|_| format!("{field} must be {N} bytes"))
}

fn decode_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|_| format!("{field} must be hexadecimal"))
}

fn validate_wallet_request_id(value: &str) -> Result<(), String> {
    let lengths = [8, 4, 4, 4, 12];
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != lengths.len()
        || parts.iter().zip(lengths).any(|(part, length)| {
            part.len() != length || !part.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err("wallet connect request ID must be an RFC 4122 UUID".into());
    }
    Ok(())
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
    use xhub_protocol_v3_6::CanonicalEncode;

    fn candidate() -> FundingCandidate {
        let network = fixed_hex::<32>(MAINNET_NETWORK_ID, "network").expect("network");
        let terms = ChannelTerms::new(
            network,
            12_288,
            200,
            6_000,
            public_key_bytes(&SecretKey::from_seed(&[1; 32])),
            public_key_bytes(&SecretKey::from_seed(&[2; 32])),
            [3; 32],
            5,
            [4; 32],
        )
        .expect("terms");
        FundingCandidate {
            protocol_version: "0x0360".into(),
            network: "mainnet".into(),
            network_id: MAINNET_NETWORK_ID.into(),
            channel_terms_hash: hex::encode(terms.hash().expect("hash")),
            channel_terms_canonical_hex: hex::encode(terms.canonical_bytes()),
            user_public_key: hex::encode(terms.user_public_key),
            user_remainder_puzzle_hash: hex::encode(terms.user_remainder_puzzle_hash),
            funding_amount_mojo: 5,
            mainnet_approved: false,
            broadcast_enabled: false,
        }
    }

    #[test]
    fn request_is_a_one_mojo_payment_with_a_positive_remainder() {
        let request = prepare_request(
            &candidate(),
            [7; 32],
            "77d3f5ad-911b-4de5-992b-6bd071e92b6f",
            &SecretKey::from_seed(&[3; 32]),
        )
        .expect("request");
        assert_eq!(request.amount, "1");
        assert_eq!(request.funding_coin_id, "07".repeat(32));
        assert!(request.user_authorization_signature.is_none());
    }

    #[test]
    fn wallet_connect_request_id_requires_uuid_text() {
        assert!(validate_wallet_request_id("77d3f5ad-911b-4de5-992b-6bd071e92b6f").is_ok());
        assert!(validate_wallet_request_id("payment-label").is_err());
    }
}
