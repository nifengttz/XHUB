use std::{env, fs, path::Path};

use chia_bls::SecretKey;
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{PROTOCOL_VERSION, public_key_bytes, sha256_parts, sign_hash};
use xhub_watchtower_v3_6::custody::CUSTODY_ATTESTATION_DOMAIN;

const CANONICAL_LENGTH: usize = 178;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttestationPayload {
    protocol_version: String,
    status: String,
    funding_coin_id: String,
    state_sequence: u64,
    entry_index: u64,
    attester_id: Option<String>,
    custody_attestation_hash: String,
    custody_attestation_canonical_hex: String,
}

#[derive(Debug, Serialize)]
struct SignedAttestationRequest {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    entry_index: u64,
    attester_id: String,
    failure_domain: String,
    attester_public_key: String,
    signature: String,
}

fn main() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err(
            "usage: custody-attest-v3-6 PAYLOAD_JSON ATTESTER_ID FAILURE_DOMAIN SECRET_FILE OUTPUT_JSON"
                .into(),
        );
    }

    let payload: AttestationPayload = serde_json::from_str(
        &fs::read_to_string(&args[0]).map_err(|error| format!("cannot read payload: {error}"))?,
    )
    .map_err(|error| format!("invalid payload JSON: {error}"))?;
    let attestation_hash = validate_payload(&payload)?;
    let secret = load_secret(Path::new(&args[3]))?;
    let signature = sign_hash(&secret, &attestation_hash);
    let request = SignedAttestationRequest {
        protocol_version: "0x0360",
        funding_coin_id: payload.funding_coin_id,
        state_sequence: payload.state_sequence,
        entry_index: payload.entry_index,
        attester_id: args[1].clone(),
        failure_domain: args[2].clone(),
        attester_public_key: hex::encode(public_key_bytes(&secret)),
        signature: hex::encode(signature),
    };
    let json = serde_json::to_string_pretty(&request).map_err(|error| error.to_string())?;
    fs::write(&args[4], format!("{json}\n")).map_err(|error| error.to_string())?;
    println!("signed_attestation={}", args[4]);
    Ok(())
}

fn validate_payload(payload: &AttestationPayload) -> Result<[u8; 32], String> {
    if payload.protocol_version != "0x0360"
        || payload.status != "SIGNING_PAYLOAD"
        || payload.attester_id.is_some()
    {
        return Err("input is not an unsigned V3.6 custody signing payload".into());
    }
    let canonical = decode_hex(
        &payload.custody_attestation_canonical_hex,
        "custody attestation canonical bytes",
    )?;
    if canonical.len() != CANONICAL_LENGTH {
        return Err(format!(
            "custody attestation canonical bytes must be {CANONICAL_LENGTH} bytes"
        ));
    }
    if u16::from_be_bytes(canonical[0..2].try_into().expect("fixed slice")) != PROTOCOL_VERSION {
        return Err("custody attestation has the wrong protocol version".into());
    }
    let funding_coin_id = fixed_hex::<32>(&payload.funding_coin_id, "funding coin ID")?;
    if canonical[2..34] != funding_coin_id {
        return Err("custody attestation funding coin ID mismatch".into());
    }
    if u64::from_be_bytes(canonical[34..42].try_into().expect("fixed slice"))
        != payload.state_sequence
        || u64::from_be_bytes(canonical[106..114].try_into().expect("fixed slice"))
            != payload.entry_index
    {
        return Err("custody attestation state or entry index mismatch".into());
    }
    let calculated = sha256_parts(&[CUSTODY_ATTESTATION_DOMAIN, &canonical]);
    if calculated
        != fixed_hex::<32>(
            &payload.custody_attestation_hash,
            "custody attestation hash",
        )?
    {
        return Err("custody attestation hash mismatch".into());
    }
    Ok(calculated)
}

fn load_secret(path: &Path) -> Result<SecretKey, String> {
    let value = fs::read_to_string(path)
        .map_err(|error| format!("cannot read attester secret: {error}"))?;
    SecretKey::from_bytes(&fixed_hex::<32>(value.trim(), "attester secret")?)
        .map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_payload_with_a_changed_hash() {
        let mut canonical = vec![0_u8; CANONICAL_LENGTH];
        canonical[0..2].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        canonical[2..34].copy_from_slice(&[1; 32]);
        canonical[34..42].copy_from_slice(&1_u64.to_be_bytes());
        let payload = AttestationPayload {
            protocol_version: "0x0360".into(),
            status: "SIGNING_PAYLOAD".into(),
            funding_coin_id: hex::encode([1; 32]),
            state_sequence: 1,
            entry_index: 0,
            attester_id: None,
            custody_attestation_hash: hex::encode([2; 32]),
            custody_attestation_canonical_hex: hex::encode(canonical),
        };
        assert_eq!(
            validate_payload(&payload).unwrap_err(),
            "custody attestation hash mismatch"
        );
    }
}
