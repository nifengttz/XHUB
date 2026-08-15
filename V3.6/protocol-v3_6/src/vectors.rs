use chia_bls::SecretKey;
use serde_json::{Value, json};

use crate::{
    BLS_CIPHERSUITE, CanonicalEncode, ChannelTerms, ConflictingResultEvidence,
    DeliveryConfirmation, DoubleSignEvidence, Ledger, LedgerEntry, MerkleProof, PROTOCOL_VERSION,
    RecoveryPackage, ReservationResult, ReservationStatus, Result, SignedReservationResult,
    StateZero, public_key_bytes, sign_hash,
};

fn repeated(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn hex32(value: [u8; 32]) -> String {
    hex::encode(value)
}

pub fn generate_golden_vectors() -> Result<Value> {
    let user_seed = repeated(0x11);
    let hub_seed = repeated(0x22);
    let merchant_seed = repeated(0x33);
    let user_secret_key = SecretKey::from_seed(&user_seed);
    let hub_secret_key = SecretKey::from_seed(&hub_seed);
    let merchant_secret_key = SecretKey::from_seed(&merchant_seed);

    let network_id = repeated(0xaa);
    let funding_coin_id = repeated(0xcc);
    let terms = ChannelTerms::new(
        network_id,
        12_288,
        200,
        6_000,
        public_key_bytes(&user_secret_key),
        public_key_bytes(&hub_secret_key),
        repeated(0xbb),
        1_000_000,
        repeated(0xdd),
    )?;
    let channel_terms_hash = terms.hash()?;
    let state_zero = StateZero::new(&terms)?;
    let state_zero_hash = state_zero.hash(&terms, &funding_coin_id)?;

    let entries = (0_u8..3)
        .map(|index| LedgerEntry {
            merchant_puzzle_hash: repeated(0xe0 + index),
            merchant_receipt_public_key: public_key_bytes(&merchant_secret_key),
            amount: 10_000 + u64::from(index) * 1_000,
            reservation_nonce: repeated(index + 1),
        })
        .collect::<Vec<_>>();
    let ledger = Ledger {
        entries: entries.clone(),
    };
    let authorization_hashes = entries
        .iter()
        .map(|entry| entry.authorization_hash(&terms, &funding_coin_id))
        .collect::<Result<Vec<_>>>()?;
    let user_signatures = authorization_hashes
        .iter()
        .map(|hash| sign_hash(&user_secret_key, hash))
        .collect::<Vec<_>>();
    let entry_hashes = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| entry.entry_hash(&terms, &funding_coin_id, index as u64))
        .collect::<Result<Vec<_>>>()?;
    let leaf_hashes = ledger.leaf_hashes(&terms, &funding_coin_id)?;
    let manifest_root = ledger.manifest_root(&terms, &funding_coin_id)?;
    let checkpoint = ledger.checkpoint(&terms, funding_coin_id, 1, state_zero_hash)?;
    let checkpoint_hash = checkpoint.hash(&terms)?;
    let hub_state_hash = checkpoint.hub_state_hash(&terms)?;
    let hub_signature = sign_hash(&hub_secret_key, &hub_state_hash);
    let official_state = crate::OfficialState {
        checkpoint,
        hub_state_signature: hub_signature,
    };
    let recovery_package = RecoveryPackage {
        funding_coin_id,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x02, 0x03],
        funding_amount: terms.funding_amount,
        channel_terms: terms.clone(),
        official_state: official_state.clone(),
        entries: entries.clone(),
        user_authorization_signatures: user_signatures.clone(),
    };
    let recovery_package_content_hash = recovery_package.content_hash()?;
    let proof = MerkleProof::build(&leaf_hashes, 2)?;
    proof.verify(leaf_hashes[2], manifest_root)?;

    let delivery = DeliveryConfirmation {
        network_id,
        funding_coin_id,
        channel_terms_hash,
        state_sequence: 1,
        checkpoint_hash,
        entry_index: 2,
        authorization_hash: authorization_hashes[2],
        recovery_package_content_hash,
    };
    let delivery_hash = delivery.hash()?;
    let delivery_signature = sign_hash(&merchant_secret_key, &delivery_hash);

    let reservation_result = ReservationResult {
        network_id,
        request_id: repeated(0x44),
        funding_coin_id,
        reservation_nonce: entries[2].reservation_nonce,
        authorization_hash: authorization_hashes[2],
        status: ReservationStatus::Signed,
        state_sequence: Some(1),
        checkpoint_hash: Some(checkpoint_hash),
        observed_peak_height: 50_000,
        acceptance_cutoff_height: 62_288,
        scheduled_close_height: 62_488,
        ledger_written: true,
    };
    let reservation_result_hash = reservation_result.hash()?;
    let reservation_result_signature = sign_hash(&hub_secret_key, &reservation_result_hash);
    let signed_reservation_result = SignedReservationResult {
        result: reservation_result.clone(),
        hub_result_signature: reservation_result_signature,
    };

    let conflicting_checkpoint = Ledger {
        entries: entries[..2].to_vec(),
    }
    .checkpoint(&terms, funding_coin_id, 1, state_zero_hash)?;
    let conflicting_official_state = crate::OfficialState {
        hub_state_signature: sign_hash(
            &hub_secret_key,
            &conflicting_checkpoint.hub_state_hash(&terms)?,
        ),
        checkpoint: conflicting_checkpoint,
    };
    let double_sign_evidence =
        DoubleSignEvidence::new(&terms, official_state.clone(), conflicting_official_state)?;
    let double_sign_evidence_hash = double_sign_evidence.hash(&terms)?;

    let conflicting_result = ReservationResult {
        request_id: repeated(0x45),
        status: ReservationStatus::InvalidAuthorization,
        state_sequence: None,
        checkpoint_hash: None,
        ledger_written: false,
        ..reservation_result.clone()
    };
    let signed_conflicting_result = SignedReservationResult {
        hub_result_signature: sign_hash(&hub_secret_key, &conflicting_result.hash()?),
        result: conflicting_result,
    };
    let conflicting_result_evidence = ConflictingResultEvidence::new(
        &terms,
        signed_reservation_result.clone(),
        signed_conflicting_result,
    )?;
    let conflicting_result_evidence_hash = conflicting_result_evidence.hash(&terms)?;

    let mut invalid_close_delay = terms.canonical_bytes();
    let close_delay_offset = 2 + 32 + 8 + 8;
    invalid_close_delay[close_delay_offset..close_delay_offset + 8]
        .copy_from_slice(&12_487_u64.to_be_bytes());
    let mut invalid_result_bool = reservation_result.canonical_bytes();
    *invalid_result_bool.last_mut().expect("non-empty result") = 2;

    Ok(json!({
        "schema": "xhub-protocol-v3-6-vectors-1",
        "protocol_version_hex": format!("{PROTOCOL_VERSION:04x}"),
        "decisions": {
            "hash": "SHA-256",
            "bls_scheme": "BLS12-381 augmented",
            "bls_ciphersuite": BLS_CIPHERSUITE,
            "integer_encoding": "unsigned big-endian fixed width",
            "array_length_encoding": "u32_be",
            "merkle_odd_leaf": "duplicate-last",
            "merkle_direction": { "0": "sibling-right", "1": "sibling-left" }
        },
        "test_seeds": {
            "warning": "TEST ONLY - never use these seeds for funds",
            "user": hex::encode(user_seed),
            "hub": hex::encode(hub_seed),
            "merchant_receipt": hex::encode(merchant_seed)
        },
        "keys": {
            "user_public_key": hex::encode(terms.user_public_key),
            "hub_public_key": hex::encode(terms.hub_state_public_key_a),
            "merchant_receipt_public_key": hex::encode(entries[0].merchant_receipt_public_key)
        },
        "channel_terms": {
            "canonical_hex": hex::encode(terms.canonical_bytes()),
            "hash": hex32(channel_terms_hash),
            "acceptance_blocks": terms.acceptance_blocks,
            "freeze_blocks": terms.freeze_blocks,
            "close_delay_blocks": terms.close_delay_blocks,
            "challenge_blocks": terms.challenge_blocks
        },
        "state_zero": {
            "manifest_root": hex32(state_zero.manifest_root),
            "hash": hex32(state_zero_hash)
        },
        "ledger": {
            "authorization_hashes": authorization_hashes.into_iter().map(hex32).collect::<Vec<_>>(),
            "user_signatures": user_signatures.iter().map(hex::encode).collect::<Vec<_>>(),
            "entry_hashes": entry_hashes.into_iter().map(hex32).collect::<Vec<_>>(),
            "leaf_hashes": leaf_hashes.into_iter().map(hex32).collect::<Vec<_>>(),
            "manifest_root": hex32(manifest_root),
            "merkle_proof_entry_2": {
                "canonical_hex": hex::encode(proof.canonical_bytes()),
                "steps": proof.steps.iter().map(|step| json!({
                    "side": step.side as u8,
                    "sibling": hex::encode(step.sibling)
                })).collect::<Vec<_>>()
            }
        },
        "official_state": {
            "checkpoint_canonical_hex": hex::encode(official_state.checkpoint.canonical_bytes()),
            "checkpoint_hash": hex32(checkpoint_hash),
            "hub_state_hash": hex32(hub_state_hash),
            "hub_state_signature": hex::encode(official_state.hub_state_signature),
            "canonical_hex": hex::encode(official_state.canonical_bytes())
        },
        "recovery_package": {
            "canonical_hex": hex::encode(recovery_package.canonical_bytes()),
            "content_hash": hex32(recovery_package_content_hash)
        },
        "delivery_confirmation": {
            "canonical_hex": hex::encode(delivery.canonical_bytes()),
            "hash": hex32(delivery_hash),
            "signature": hex::encode(delivery_signature)
        },
        "reservation_result": {
            "canonical_hex": hex::encode(reservation_result.canonical_bytes()),
            "hash": hex32(reservation_result_hash),
            "signature": hex::encode(reservation_result_signature),
            "signed_canonical_hex": hex::encode(signed_reservation_result.canonical_bytes())
        },
        "double_sign_evidence": {
            "canonical_hex": hex::encode(double_sign_evidence.canonical_bytes()),
            "hash": hex32(double_sign_evidence_hash)
        },
        "conflicting_result_evidence": {
            "canonical_hex": hex::encode(conflicting_result_evidence.canonical_bytes()),
            "hash": hex32(conflicting_result_evidence_hash)
        },
        "negative_vectors": {
            "public_key_infinity": format!("c0{}", "00".repeat(47)),
            "signature_infinity": format!("c0{}", "00".repeat(95)),
            "channel_terms_close_delay_mismatch": hex::encode(invalid_close_delay),
            "reservation_result_invalid_bool": hex::encode(invalid_result_bool)
        }
    }))
}
