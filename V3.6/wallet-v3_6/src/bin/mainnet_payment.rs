use std::{env, fs, path::PathBuf};

use chia_bls::SecretKey;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{
    CanonicalDecode, CanonicalEncode, ChannelTerms, DeliveryConfirmation, LedgerEntry,
    RecoveryPackage, public_key_bytes, sign_hash, verify_hash,
};

const CHIA_MAINNET_GENESIS_CHALLENGE: [u8; 32] = [
    0xcc, 0xd5, 0xbb, 0x71, 0x18, 0x35, 0x32, 0xbf, 0xf2, 0x20, 0xba, 0x46, 0xc2, 0x68, 0x99, 0x1a,
    0x3f, 0xf0, 0x7e, 0xb3, 0x58, 0xe8, 0x25, 0x5a, 0x65, 0xc3, 0x0a, 0x2d, 0xce, 0x0e, 0x5f, 0xbb,
];

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Serialize)]
struct Confirmer {
    signer_id: &'static str,
    failure_domain: &'static str,
    signer_public_key: String,
}

#[derive(Serialize)]
struct SignedConfirmation {
    protocol_version: &'static str,
    signer_id: &'static str,
    failure_domain: &'static str,
    signer_public_key: String,
    delivery_confirmation_canonical_hex: String,
    signature: String,
}

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("prepare") => prepare(args.collect()),
        Some("verify-payment") => verify_payment(args.collect()),
        Some("confirm-delivery") => confirm_delivery(args.collect()),
        _ => Err("usage: mainnet-payment prepare EXPERIMENT RESULT OUTPUT SECRET CONFIRMERS | verify-payment SIGNED_REQUEST HUB_REQUEST | confirm-delivery PACKAGE OUTPUT SECRET".into()),
    }
}

fn prepare(args: Vec<String>) -> Result<(), String> {
    if args.len() != 5 {
        return Err("prepare requires EXPERIMENT RESULT OUTPUT SECRET CONFIRMERS".into());
    }
    let experiment: serde_json::Value = read_json(&args[0])?;
    let result: serde_json::Value = read_json(&args[1])?;
    require_experiment_guard(&experiment, &result)?;
    let terms_hex = text(&experiment, "channel_terms_canonical_hex")?;
    let terms = ChannelTerms::from_canonical_bytes(&decode_hex(terms_hex, "channel terms")?)
        .map_err(|error| error.to_string())?;
    let funding_coin_id = fixed_hex(text(&result, "coin_id")?, "funding coin id")?;
    let merchant_secret = load_or_generate_secret(&PathBuf::from(&args[3]))?;
    let merchant_puzzle_hash = fixed_hex(
        text(&experiment, "user_remainder_puzzle_hash")?,
        "merchant puzzle hash",
    )?;
    let entry = LedgerEntry {
        merchant_puzzle_hash,
        merchant_receipt_public_key: public_key_bytes(&merchant_secret),
        amount: 1,
        reservation_nonce: random_bytes32(),
    };
    let request = PaymentRequest {
        schema: "xhub-v3-6-mainnet-payment-request-1".into(),
        protocol_version: "0x0360".into(),
        network: "mainnet".into(),
        network_id: hex::encode(terms.network_id),
        channel_terms_canonical_hex: terms_hex.to_string(),
        channel_terms_hash: hex::encode(terms.hash().map_err(|error| error.to_string())?),
        wallet_request_id: text(&experiment, "wallet_request_id")?.to_string(),
        request_id: hex::encode(random_bytes32()),
        funding_coin_id: hex::encode(funding_coin_id),
        merchant_puzzle_hash: hex::encode(entry.merchant_puzzle_hash),
        merchant_receipt_public_key: hex::encode(entry.merchant_receipt_public_key),
        amount: "1".into(),
        reservation_nonce: hex::encode(entry.reservation_nonce),
        authorization_hash: hex::encode(
            entry
                .authorization_hash(&terms, &funding_coin_id)
                .map_err(|error| error.to_string())?,
        ),
        user_public_key: hex::encode(terms.user_public_key),
        user_authorization_signature: None,
    };
    write_json(&args[2], &request)?;
    write_json(
        &args[4],
        &vec![Confirmer {
            signer_id: "merchant-mainnet-experiment-1",
            failure_domain: "local-mainnet-experiment",
            signer_public_key: hex::encode(public_key_bytes(&merchant_secret)),
        }],
    )?;
    println!("authorization_hash={}", request.authorization_hash);
    println!("payment_request={}", args[2]);
    Ok(())
}

fn verify_payment(args: Vec<String>) -> Result<(), String> {
    if args.len() != 2 {
        return Err("verify-payment requires SIGNED_REQUEST HUB_REQUEST".into());
    }
    let request: PaymentRequest =
        serde_json::from_str(&fs::read_to_string(&args[0]).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid signed payment JSON: {error}"))?;
    let hub_request = validate_signed_payment(&request)?;
    write_json(&args[1], &hub_request)?;
    println!("authorization_hash={}", request.authorization_hash);
    println!("hub_request={}", args[1]);
    Ok(())
}

fn validate_signed_payment(request: &PaymentRequest) -> Result<HubReservationRequest, String> {
    if request.schema != "xhub-v3-6-mainnet-payment-request-1"
        || request.protocol_version != "0x0360"
        || request.network != "mainnet"
    {
        return Err(
            "payment schema, protocol version, or network is not the mainnet V3.6 experiment"
                .into(),
        );
    }
    if request.amount != "1" {
        return Err("mainnet experiment payment amount must be exactly 1 mojo".into());
    }
    if request.wallet_request_id.is_empty() {
        return Err("wallet_request_id is required to reconstruct the channel key".into());
    }
    let terms = ChannelTerms::from_canonical_bytes(&decode_hex(
        &request.channel_terms_canonical_hex,
        "channel terms",
    )?)
    .map_err(|error| error.to_string())?;
    if terms.network_id != CHIA_MAINNET_GENESIS_CHALLENGE || !matches!(terms.funding_amount, 5 | 10)
    {
        return Err(
            "channel terms are not a supported 5/10 mojo Chia mainnet recovery canary or experiment"
                .into(),
        );
    }
    fixed_hex::<32>(&request.request_id, "request id")?;
    let funding_coin_id = fixed_hex(&request.funding_coin_id, "funding coin id")?;
    let entry = LedgerEntry {
        merchant_puzzle_hash: fixed_hex(&request.merchant_puzzle_hash, "merchant puzzle hash")?,
        merchant_receipt_public_key: fixed_hex(
            &request.merchant_receipt_public_key,
            "merchant receipt public key",
        )?,
        amount: 1,
        reservation_nonce: fixed_hex(&request.reservation_nonce, "reservation nonce")?,
    };
    let expected_terms_hash = terms.hash().map_err(|error| error.to_string())?;
    let expected_authorization_hash = entry
        .authorization_hash(&terms, &funding_coin_id)
        .map_err(|error| error.to_string())?;
    if fixed_hex::<32>(&request.network_id, "network id")? != terms.network_id
        || fixed_hex::<32>(&request.channel_terms_hash, "channel terms hash")?
            != expected_terms_hash
        || fixed_hex::<48>(&request.user_public_key, "user public key")? != terms.user_public_key
        || fixed_hex::<32>(&request.authorization_hash, "authorization hash")?
            != expected_authorization_hash
    {
        return Err("signed payment fields do not match their canonical V3.6 values".into());
    }
    if entry.merchant_puzzle_hash != terms.user_remainder_puzzle_hash {
        return Err("merchant target is not the experiment wallet remainder puzzle hash".into());
    }
    let signature_hex = request
        .user_authorization_signature
        .as_deref()
        .ok_or("wallet authorization signature is missing")?;
    let signature = fixed_hex::<96>(signature_hex, "user authorization signature")?;
    verify_hash(
        &terms.user_public_key,
        &expected_authorization_hash,
        &signature,
    )
    .map_err(|error| format!("wallet authorization signature is invalid: {error}"))?;

    Ok(HubReservationRequest {
        protocol_version: request.protocol_version.clone(),
        request_id: request.request_id.clone(),
        funding_coin_id: request.funding_coin_id.clone(),
        merchant_puzzle_hash: request.merchant_puzzle_hash.clone(),
        merchant_receipt_public_key: request.merchant_receipt_public_key.clone(),
        amount: request.amount.clone(),
        reservation_nonce: request.reservation_nonce.clone(),
        user_authorization_signature: signature_hex.to_string(),
    })
}

fn confirm_delivery(args: Vec<String>) -> Result<(), String> {
    if args.len() != 3 {
        return Err("confirm-delivery requires PACKAGE OUTPUT SECRET".into());
    }
    let package_json: serde_json::Value = read_json(&args[0])?;
    let package_hex = text(&package_json, "recovery_package_canonical_hex")?;
    let package = RecoveryPackage::from_canonical_bytes(&decode_hex(package_hex, "package")?)
        .map_err(|error| error.to_string())?;
    package.validate().map_err(|error| error.to_string())?;
    if package.entries.len() != 1 {
        return Err("mainnet experiment expects exactly one ledger entry".into());
    }
    let merchant_secret = load_secret(&PathBuf::from(&args[2]))?;
    if public_key_bytes(&merchant_secret) != package.entries[0].merchant_receipt_public_key {
        return Err("merchant secret does not match package receipt public key".into());
    }
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| error.to_string())?;
    let confirmation = DeliveryConfirmation {
        network_id: package.channel_terms.network_id,
        funding_coin_id: package.funding_coin_id,
        channel_terms_hash: package
            .channel_terms
            .hash()
            .map_err(|error| error.to_string())?,
        state_sequence: package.official_state.checkpoint.state_sequence,
        checkpoint_hash,
        entry_index: 0,
        authorization_hash: package.entries[0]
            .authorization_hash(&package.channel_terms, &package.funding_coin_id)
            .map_err(|error| error.to_string())?,
        recovery_package_content_hash: package.content_hash().map_err(|error| error.to_string())?,
    };
    let signature = sign_hash(
        &merchant_secret,
        &confirmation.hash().map_err(|error| error.to_string())?,
    );
    write_json(
        &args[1],
        &SignedConfirmation {
            protocol_version: "0x0360",
            signer_id: "merchant-mainnet-experiment-1",
            failure_domain: "local-mainnet-experiment",
            signer_public_key: hex::encode(public_key_bytes(&merchant_secret)),
            delivery_confirmation_canonical_hex: hex::encode(confirmation.canonical_bytes()),
            signature: hex::encode(signature),
        },
    )?;
    println!("confirmation={}", args[1]);
    Ok(())
}

fn require_experiment_guard(
    experiment: &serde_json::Value,
    result: &serde_json::Value,
) -> Result<(), String> {
    if experiment.get("network").and_then(|v| v.as_str()) != Some("mainnet")
        || result.get("network").and_then(|v| v.as_str()) != Some("mainnet")
        || experiment.get("mainnet_approved").and_then(|v| v.as_bool()) != Some(false)
        || result.get("mainnet_approved").and_then(|v| v.as_bool()) != Some(false)
        || result
            .pointer("/hub_registration/chain_state")
            .and_then(|v| v.as_str())
            != Some("ACTIVE")
    {
        return Err("mainnet experiment guard or ACTIVE registration is missing".into());
    }
    Ok(())
}

fn random_bytes32() -> [u8; 32] {
    let mut value = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut value);
    value
}

fn load_or_generate_secret(path: &PathBuf) -> Result<SecretKey, String> {
    if path.exists() {
        return load_secret(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let secret = SecretKey::from_seed(&random_bytes32());
    fs::write(path, format!("{}\n", hex::encode(secret.to_bytes())))
        .map_err(|error| error.to_string())?;
    Ok(secret)
}

fn load_secret(path: &PathBuf) -> Result<SecretKey, String> {
    let bytes: [u8; 32] = decode_hex(
        fs::read_to_string(path)
            .map_err(|error| error.to_string())?
            .trim(),
        "merchant secret",
    )?
    .try_into()
    .map_err(|_| "merchant secret must be 32 bytes")?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn read_json(path: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(&fs::read_to_string(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn write_json(path: &str, value: &impl Serialize) -> Result<(), String> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n")).map_err(|error| error.to_string())
}

fn text<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .ok_or_else(|| format!("missing {field}"))
}

fn decode_hex(value: &str, field: &str) -> Result<Vec<u8>, String> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| format!("invalid {field}: {error}"))
}

fn fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    decode_hex(value, field)?
        .try_into()
        .map_err(|_| format!("{field} must be {N} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xhub_protocol_v3_6::state_rules_hash;
    use xhub_puzzles_v3_6::module_hashes;

    fn signed_request(funding_amount: u64) -> PaymentRequest {
        let user_secret = SecretKey::from_seed(&[7_u8; 32]);
        let merchant_secret = SecretKey::from_seed(&[8_u8; 32]);
        let modules = module_hashes();
        let terms = ChannelTerms {
            network_id: CHIA_MAINNET_GENESIS_CHALLENGE,
            acceptance_blocks: 12_288,
            freeze_blocks: 200,
            close_delay_blocks: 12_488,
            challenge_blocks: 6_000,
            user_public_key: public_key_bytes(&user_secret),
            hub_state_public_key_a: public_key_bytes(&SecretKey::from_seed(&[9_u8; 32])),
            state_rules_hash: state_rules_hash(
                &modules.initial_closing,
                &modules.subsequent_closing,
                &modules.merchant_payment,
            ),
            funding_amount,
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
            user_authorization_signature: Some(hex::encode(sign_hash(
                &user_secret,
                &authorization_hash,
            ))),
        }
    }

    #[test]
    fn verifies_exact_mainnet_experiment_payment_and_rejects_tampering() {
        let request = signed_request(10);
        let hub_request = validate_signed_payment(&request).expect("valid signed request");
        assert_eq!(hub_request.amount, "1");

        let mut wrong_amount = signed_request(10);
        wrong_amount.amount = "2".into();
        assert!(validate_signed_payment(&wrong_amount).is_err());

        let mut wrong_signature = signed_request(10);
        wrong_signature.user_authorization_signature = Some("00".repeat(96));
        assert!(validate_signed_payment(&wrong_signature).is_err());

        let recovery_canary = signed_request(5);
        assert!(validate_signed_payment(&recovery_canary).is_ok());

        let unsupported_amount = signed_request(6);
        assert!(validate_signed_payment(&unsupported_amount).is_err());
    }
}
