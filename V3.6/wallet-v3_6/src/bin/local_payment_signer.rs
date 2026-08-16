//! A deliberately local-only signer for the V3.6 5-mojo recovery canary.
//!
//! This binary never opens a network connection, builds a SpendBundle, or broadcasts.
//! The user secret is read only from a local file selected by the operator and is never
//! serialized or printed.

use std::{env, fs, fs::OpenOptions, io::Write, path::Path};

use chia_bls::SecretKey;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{
    CanonicalDecode, ChannelTerms, LedgerEntry, public_key_bytes, sign_hash, verify_hash,
};

const MAINNET_NETWORK_ID: &str = "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";
const OFFCHAIN_CONFIRMATION: &str = "--confirm-offchain-1-mojo";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PaymentRequest {
    schema: String,
    protocol_version: String,
    network: String,
    network_id: String,
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

#[derive(Debug, Serialize)]
struct SigningReview {
    schema: &'static str,
    protocol_version: &'static str,
    network: &'static str,
    funding_coin_id: String,
    funding_amount_mojo: u64,
    offchain_reservation_amount_mojo: u64,
    user_remainder_amount_mojo: u64,
    merchant_puzzle_hash: String,
    reservation_nonce: String,
    request_id: String,
    authorization_hash: String,
    signature_status: &'static str,
    local_only: bool,
    spend_bundle_created: bool,
    push_tx_called: bool,
    chain_broadcast: bool,
}

struct ValidatedRequest {
    request: PaymentRequest,
    terms: ChannelTerms,
    authorization_hash: [u8; 32],
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("inspect") if args.len() == 2 => {
            let validated = validate_request(&read_request(&args[1])?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&review(&validated, signature_status(&validated)?))
                    .map_err(|error| error.to_string())?
            );
            Ok(())
        }
        Some("sign") if args.len() == 5 && args[4] == OFFCHAIN_CONFIRMATION => {
            sign_request(&args[1], &args[2], &args[3])
        }
        _ => Err(format!(
            "usage: xhub-local-signer-v3-6 inspect REQUEST_JSON | sign REQUEST_JSON USER_SECRET_FILE OUTPUT_JSON {OFFCHAIN_CONFIRMATION}"
        )),
    }
}

fn sign_request(request_path: &str, secret_path: &str, output_path: &str) -> Result<(), String> {
    let mut validated = validate_request(&read_request(request_path)?)?;
    if validated.request.user_authorization_signature.is_some() {
        return Err("refusing to replace an existing user authorization signature".into());
    }
    let secret = load_user_secret(Path::new(secret_path))?;
    if public_key_bytes(&secret) != validated.terms.user_public_key {
        return Err("the selected local secret does not match the request user_public_key".into());
    }

    validated.request.user_authorization_signature = Some(hex::encode(sign_hash(
        &secret,
        &validated.authorization_hash,
    )));
    verify_signature(&validated)?;
    write_new_json(Path::new(output_path), &validated.request)?;

    let review = review(&validated, "SIGNED_LOCAL_ONLY");
    println!(
        "{}",
        serde_json::to_string_pretty(&review).map_err(|error| error.to_string())?
    );
    println!("signed_request={output_path}");
    Ok(())
}

fn read_request(path: &str) -> Result<PaymentRequest, String> {
    serde_json::from_str(&fs::read_to_string(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid payment request JSON: {error}"))
}

fn validate_request(request: &PaymentRequest) -> Result<ValidatedRequest, String> {
    if request.schema != "xhub-v3-6-mainnet-payment-request-1"
        || request.protocol_version != "0x0360"
        || request.network != "mainnet"
        || request.amount != "1"
    {
        return Err("request is not an exact 1-mojo V3.6 mainnet payment authorization".into());
    }
    if request.wallet_request_id.is_empty() {
        return Err("wallet_request_id is required".into());
    }

    let terms = ChannelTerms::from_canonical_bytes(&decode_hex(
        &request.channel_terms_canonical_hex,
        "channel terms",
    )?)
    .map_err(|error| format!("invalid channel terms: {error}"))?;
    if terms.network_id != fixed_hex::<32>(MAINNET_NETWORK_ID, "mainnet network ID")?
        || terms.funding_amount != 5
        || fixed_hex::<32>(&request.network_id, "network ID")? != terms.network_id
        || fixed_hex::<32>(&request.channel_terms_hash, "channel terms hash")?
            != terms.hash().map_err(|error| error.to_string())?
        || fixed_hex::<48>(&request.user_public_key, "user public key")? != terms.user_public_key
    {
        return Err(
            "request does not bind to the V3.6 5-mojo mainnet recovery canary terms".into(),
        );
    }

    let funding_coin_id = fixed_hex::<32>(&request.funding_coin_id, "funding coin ID")?;
    let entry = LedgerEntry {
        merchant_puzzle_hash: fixed_hex(&request.merchant_puzzle_hash, "merchant puzzle hash")?,
        merchant_receipt_public_key: fixed_hex(
            &request.merchant_receipt_public_key,
            "merchant receipt public key",
        )?,
        amount: 1,
        reservation_nonce: fixed_hex(&request.reservation_nonce, "reservation nonce")?,
    };
    if entry.merchant_puzzle_hash != terms.user_remainder_puzzle_hash {
        return Err(
            "merchant target is not the channel's declared user remainder puzzle hash".into(),
        );
    }
    fixed_hex::<32>(&request.request_id, "request ID")?;
    let authorization_hash = entry
        .authorization_hash(&terms, &funding_coin_id)
        .map_err(|error| error.to_string())?;
    if fixed_hex::<32>(&request.authorization_hash, "authorization hash")? != authorization_hash {
        return Err("authorization hash does not match the canonical V3.6 request fields".into());
    }

    Ok(ValidatedRequest {
        request: request.clone(),
        terms,
        authorization_hash,
    })
}

fn signature_status(validated: &ValidatedRequest) -> Result<&'static str, String> {
    if validated.request.user_authorization_signature.is_none() {
        return Ok("UNSIGNED");
    }
    verify_signature(validated)?;
    Ok("VALID")
}

fn verify_signature(validated: &ValidatedRequest) -> Result<(), String> {
    let signature = fixed_hex::<96>(
        validated
            .request
            .user_authorization_signature
            .as_deref()
            .ok_or("user authorization signature is missing")?,
        "user authorization signature",
    )?;
    verify_hash(
        &validated.terms.user_public_key,
        &validated.authorization_hash,
        &signature,
    )
    .map_err(|error| format!("invalid user authorization signature: {error}"))
}

fn review(validated: &ValidatedRequest, signature_status: &'static str) -> SigningReview {
    SigningReview {
        schema: "xhub-v3-6-local-signing-review-1",
        protocol_version: "0x0360",
        network: "mainnet",
        funding_coin_id: validated.request.funding_coin_id.clone(),
        funding_amount_mojo: validated.terms.funding_amount,
        offchain_reservation_amount_mojo: 1,
        user_remainder_amount_mojo: validated.terms.funding_amount - 1,
        merchant_puzzle_hash: validated.request.merchant_puzzle_hash.clone(),
        reservation_nonce: validated.request.reservation_nonce.clone(),
        request_id: validated.request.request_id.clone(),
        authorization_hash: validated.request.authorization_hash.clone(),
        signature_status,
        local_only: true,
        spend_bundle_created: false,
        push_tx_called: false,
        chain_broadcast: false,
    }
}

fn load_user_secret(path: &Path) -> Result<SecretKey, String> {
    let bytes: [u8; 32] = decode_hex(
        fs::read_to_string(path)
            .map_err(|error| format!("cannot read local user secret: {error}"))?
            .trim(),
        "local user secret",
    )?
    .try_into()
    .map_err(|_| "local user secret must be exactly 32 bytes of hexadecimal".to_string())?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn write_new_json(path: &Path, value: &PaymentRequest) -> Result<(), String> {
    if path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .is_some_and(|parent| !parent.exists())
    {
        return Err(
            "output directory does not exist; refusing to create paths around a signing artifact"
                .into(),
        );
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("refusing to overwrite signed output: {error}"))?;
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    output
        .write_all(format!("{json}\n").as_bytes())
        .map_err(|error| error.to_string())
}

fn decode_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|_| format!("{field} must be hexadecimal"))
}

fn fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    decode_hex(value, field)?
        .try_into()
        .map_err(|_| format!("{field} must be {N} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xhub_protocol_v3_6::{CanonicalEncode, state_rules_hash};
    use xhub_puzzles_v3_6::module_hashes;

    fn unsigned_request() -> (PaymentRequest, SecretKey) {
        let user_secret = SecretKey::from_seed(&[7; 32]);
        let merchant_secret = SecretKey::from_seed(&[8; 32]);
        let modules = module_hashes();
        let terms = ChannelTerms {
            network_id: fixed_hex(MAINNET_NETWORK_ID, "network").expect("network"),
            acceptance_blocks: 12_288,
            freeze_blocks: 200,
            close_delay_blocks: 12_488,
            challenge_blocks: 6_000,
            user_public_key: public_key_bytes(&user_secret),
            hub_state_public_key_a: public_key_bytes(&SecretKey::from_seed(&[9; 32])),
            state_rules_hash: state_rules_hash(
                &modules.initial_closing,
                &modules.subsequent_closing,
                &modules.merchant_payment,
            ),
            funding_amount: 5,
            user_remainder_puzzle_hash: [0xaa; 32],
            max_ledger_entries: 64,
        };
        let funding_coin_id = [0xbb; 32];
        let entry = LedgerEntry {
            merchant_puzzle_hash: terms.user_remainder_puzzle_hash,
            merchant_receipt_public_key: public_key_bytes(&merchant_secret),
            amount: 1,
            reservation_nonce: [0xcc; 32],
        };
        let authorization_hash = entry
            .authorization_hash(&terms, &funding_coin_id)
            .expect("authorization hash");
        (
            PaymentRequest {
                schema: "xhub-v3-6-mainnet-payment-request-1".into(),
                protocol_version: "0x0360".into(),
                network: "mainnet".into(),
                network_id: hex::encode(terms.network_id),
                channel_terms_canonical_hex: hex::encode(terms.canonical_bytes()),
                channel_terms_hash: hex::encode(terms.hash().expect("terms hash")),
                wallet_request_id: "77d3f5ad-911b-4de5-992b-6bd071e92b6f".into(),
                request_id: hex::encode([0xdd; 32]),
                funding_coin_id: hex::encode(funding_coin_id),
                merchant_puzzle_hash: hex::encode(entry.merchant_puzzle_hash),
                merchant_receipt_public_key: hex::encode(entry.merchant_receipt_public_key),
                amount: "1".into(),
                reservation_nonce: hex::encode(entry.reservation_nonce),
                authorization_hash: hex::encode(authorization_hash),
                user_public_key: hex::encode(terms.user_public_key),
                user_authorization_signature: None,
            },
            user_secret,
        )
    }

    #[test]
    fn validates_and_signs_exact_one_mojo_request() {
        let (mut request, secret) = unsigned_request();
        let validated = validate_request(&request).expect("valid unsigned request");
        assert_eq!(signature_status(&validated).expect("status"), "UNSIGNED");
        request.user_authorization_signature = Some(hex::encode(sign_hash(
            &secret,
            &validated.authorization_hash,
        )));
        let signed = validate_request(&request).expect("valid signed request");
        assert_eq!(signature_status(&signed).expect("status"), "VALID");
        assert!(!review(&signed, "VALID").chain_broadcast);
    }

    #[test]
    fn rejects_changed_amount_and_wrong_key() {
        let (mut request, _) = unsigned_request();
        request.amount = "2".into();
        assert!(validate_request(&request).is_err());

        let (mut request, _) = unsigned_request();
        let validated = validate_request(&request).expect("valid request");
        request.user_authorization_signature = Some(hex::encode(sign_hash(
            &SecretKey::from_seed(&[42; 32]),
            &validated.authorization_hash,
        )));
        assert!(signature_status(&validate_request(&request).expect("canonical request")).is_err());
    }
}
