use std::fs;

use chia_bls::SecretKey;
use serde_json::Value;
use xhub_protocol_v3_6::{
    BLS_CIPHERSUITE, CanonicalDecode, CanonicalEncode, ChannelTerms, ConflictingResultEvidence,
    DeliveryConfirmation, DoubleSignEvidence, Ledger, LedgerEntry, MerkleProof, ProtocolError,
    RecoveryPackage, ReservationResult, StateZero, generate_golden_vectors, parse_public_key,
    parse_signature, sha256_parts, verify_hash,
};

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/protocol-v3_6.json"
    );
    serde_json::from_str(&fs::read_to_string(path).expect("golden vectors must exist"))
        .expect("golden vectors must be valid JSON")
}

fn hex_at(root: &Value, pointer: &str) -> Vec<u8> {
    hex::decode(
        root.pointer(pointer)
            .and_then(Value::as_str)
            .expect("hex vector"),
    )
    .expect("hex vector must decode")
}

fn array48(value: &[u8]) -> [u8; 48] {
    value.try_into().expect("expected public key")
}

fn array96(value: &[u8]) -> [u8; 96] {
    value.try_into().expect("expected signature")
}

#[test]
fn committed_vectors_are_deterministic() {
    let committed = vectors();
    let generated = generate_golden_vectors().expect("vector generation");
    assert_eq!(committed, generated);
    assert_eq!(committed["decisions"]["bls_ciphersuite"], BLS_CIPHERSUITE);
}

#[test]
fn core_objects_round_trip_canonically() {
    let committed = vectors();
    let terms_bytes = hex_at(&committed, "/channel_terms/canonical_hex");
    let terms = ChannelTerms::from_canonical_bytes(&terms_bytes).expect("terms decode");
    assert_eq!(
        hex::encode(terms.canonical_bytes()),
        committed["channel_terms"]["canonical_hex"]
    );

    let package_bytes = hex_at(&committed, "/recovery_package/canonical_hex");
    let package = RecoveryPackage::from_canonical_bytes(&package_bytes).expect("package decode");
    package.validate().expect("package validation");
    assert_eq!(
        hex::encode(package.canonical_bytes()),
        committed["recovery_package"]["canonical_hex"]
    );

    let delivery_bytes = hex_at(&committed, "/delivery_confirmation/canonical_hex");
    let delivery =
        DeliveryConfirmation::from_canonical_bytes(&delivery_bytes).expect("delivery decode");
    assert_eq!(
        hex::encode(delivery.canonical_bytes()),
        committed["delivery_confirmation"]["canonical_hex"]
    );

    let result_bytes = hex_at(&committed, "/reservation_result/canonical_hex");
    let result = ReservationResult::from_canonical_bytes(&result_bytes).expect("result decode");
    assert_eq!(
        hex::encode(result.canonical_bytes()),
        committed["reservation_result"]["canonical_hex"]
    );

    let double_sign_bytes = hex_at(&committed, "/double_sign_evidence/canonical_hex");
    let double_sign = DoubleSignEvidence::from_canonical_bytes(&double_sign_bytes)
        .expect("double-sign evidence decode");
    double_sign
        .validate(&terms)
        .expect("double-sign evidence validation");
    assert_eq!(
        hex::encode(double_sign.canonical_bytes()),
        committed["double_sign_evidence"]["canonical_hex"]
    );

    let conflicting_result_bytes = hex_at(&committed, "/conflicting_result_evidence/canonical_hex");
    let conflicting_result =
        ConflictingResultEvidence::from_canonical_bytes(&conflicting_result_bytes)
            .expect("conflicting-result evidence decode");
    conflicting_result
        .validate(&terms)
        .expect("conflicting-result evidence validation");
    assert_eq!(
        hex::encode(conflicting_result.canonical_bytes()),
        committed["conflicting_result_evidence"]["canonical_hex"]
    );
}

#[test]
fn canonical_decoders_reject_trailing_bytes() {
    let committed = vectors();
    let mut bytes = hex_at(&committed, "/channel_terms/canonical_hex");
    bytes.push(0);
    assert_eq!(
        ChannelTerms::from_canonical_bytes(&bytes),
        Err(ProtocolError::TrailingBytes(1))
    );
}

#[test]
fn invalid_bls_points_are_rejected() {
    let committed = vectors();
    let public_key = hex_at(&committed, "/negative_vectors/public_key_infinity");
    let signature = hex_at(&committed, "/negative_vectors/signature_infinity");
    assert_eq!(
        parse_public_key(&array48(&public_key)),
        Err(ProtocolError::PublicKeyInfinity)
    );
    assert_eq!(
        parse_signature(&array96(&signature)),
        Err(ProtocolError::SignatureInfinity)
    );
}

#[test]
fn invalid_close_delay_and_option_tag_are_rejected() {
    let committed = vectors();
    let terms = hex_at(
        &committed,
        "/negative_vectors/channel_terms_close_delay_mismatch",
    );
    assert_eq!(
        ChannelTerms::from_canonical_bytes(&terms),
        Err(ProtocolError::CloseDelayMismatch)
    );
    let result = hex_at(
        &committed,
        "/negative_vectors/reservation_result_invalid_bool",
    );
    assert_eq!(
        ReservationResult::from_canonical_bytes(&result),
        Err(ProtocolError::InvalidBool(2))
    );
}

#[test]
fn merkle_proofs_verify_for_empty_and_odd_sizes() {
    assert_eq!(
        xhub_protocol_v3_6::empty_root(),
        sha256_parts(&[xhub_protocol_v3_6::LEDGER_EMPTY_DOMAIN])
    );
    for count in [1_usize, 2, 3, 10, 64] {
        let leaves = (0..count)
            .map(|index| sha256_parts(&[b"test-leaf", &(index as u64).to_be_bytes()]))
            .collect::<Vec<_>>();
        let root = xhub_protocol_v3_6::merkle_root(&leaves);
        for index in 0..count {
            let proof = MerkleProof::build(&leaves, index).expect("proof");
            proof
                .verify(leaves[index], root)
                .expect("proof verification");
            let mut encoded = proof.canonical_bytes();
            let decoded = MerkleProof::from_canonical_bytes(&encoded).expect("proof decode");
            decoded.verify(leaves[index], root).expect("decoded proof");
            if proof.steps.is_empty() {
                let wrong_leaf = sha256_parts(&[b"wrong-single-leaf"]);
                assert!(proof.verify(wrong_leaf, root).is_err());
            } else {
                let last = encoded.len() - 1;
                encoded[last] ^= 1;
                let tampered =
                    MerkleProof::from_canonical_bytes(&encoded).expect("tampered decode");
                assert!(tampered.verify(leaves[index], root).is_err());
            }
        }
    }
}

#[test]
fn duplicate_nonces_and_amount_overruns_are_rejected() {
    let committed = vectors();
    let package_bytes = hex_at(&committed, "/recovery_package/canonical_hex");
    let mut package = RecoveryPackage::from_canonical_bytes(&package_bytes).expect("package");
    package.entries[1].reservation_nonce = package.entries[0].reservation_nonce;
    assert_eq!(package.validate(), Err(ProtocolError::DuplicateNonce));

    let package = RecoveryPackage::from_canonical_bytes(&package_bytes).expect("package");
    let ledger = Ledger {
        entries: vec![
            LedgerEntry {
                amount: package.channel_terms.funding_amount,
                ..package.entries[0].clone()
            },
            LedgerEntry {
                amount: 1,
                reservation_nonce: [0xf0; 32],
                ..package.entries[1].clone()
            },
        ],
    };
    assert_eq!(
        ledger.validate(&package.channel_terms),
        Err(ProtocolError::InsufficientRemainder)
    );
}

#[test]
fn same_merchant_and_amount_still_produce_distinct_entry_hashes() {
    let committed = vectors();
    let package_bytes = hex_at(&committed, "/recovery_package/canonical_hex");
    let package = RecoveryPackage::from_canonical_bytes(&package_bytes).expect("package");
    let mut first = package.entries[0].clone();
    let mut second = first.clone();
    second.reservation_nonce = [0xa5; 32];
    let first_hash = first
        .entry_hash(&package.channel_terms, &package.funding_coin_id, 0)
        .expect("first hash");
    let second_hash = second
        .entry_hash(&package.channel_terms, &package.funding_coin_id, 1)
        .expect("second hash");
    assert_ne!(first_hash, second_hash);
    first.amount = second.amount;
    assert_ne!(
        first
            .entry_hash(&package.channel_terms, &package.funding_coin_id, 0)
            .expect("first hash"),
        second_hash
    );
}

#[test]
fn state_zero_and_signatures_match_vectors() {
    let committed = vectors();
    let package = RecoveryPackage::from_canonical_bytes(&hex_at(
        &committed,
        "/recovery_package/canonical_hex",
    ))
    .expect("package");
    let state_zero = StateZero::new(&package.channel_terms).expect("state zero");
    let state_zero_hash = state_zero
        .hash(&package.channel_terms, &package.funding_coin_id)
        .expect("state zero hash");
    assert_eq!(
        hex::encode(state_zero_hash),
        committed["state_zero"]["hash"]
    );
    package
        .official_state
        .verify(&package.channel_terms)
        .expect("hub signature");

    let user_signature = array96(&hex_at(&committed, "/ledger/user_signatures/2"));
    let authorization_hash = package.entries[2]
        .authorization_hash(&package.channel_terms, &package.funding_coin_id)
        .expect("authorization hash");
    verify_hash(
        &package.channel_terms.user_public_key,
        &authorization_hash,
        &user_signature,
    )
    .expect("user signature");

    let unused = SecretKey::from_seed(&[0x44; 32]);
    assert_ne!(
        unused.public_key().to_bytes(),
        package.channel_terms.user_public_key
    );
}
