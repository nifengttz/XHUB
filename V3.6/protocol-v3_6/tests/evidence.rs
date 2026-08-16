use chia_bls::SecretKey;
use xhub_protocol_v3_6::{
    CanonicalDecode, CanonicalEncode, ChannelTerms, ConflictingResultEvidence, DoubleSignEvidence,
    Ledger, LedgerEntry, OfficialState, ProtocolError, ReservationResult, ReservationStatus,
    SignedReservationResult, StateZero, public_key_bytes, sign_hash,
};

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn terms() -> ChannelTerms {
    ChannelTerms::new(
        [0xaa; 32],
        100,
        10,
        50,
        public_key_bytes(&key(1)),
        public_key_bytes(&key(2)),
        [0x36; 32],
        1_000,
        [0x77; 32],
    )
    .expect("terms")
}

fn signed_state(ledger: Ledger) -> OfficialState {
    let terms = terms();
    let coin_id = [0x42; 32];
    let previous = StateZero::new(&terms)
        .expect("state zero")
        .hash(&terms, &coin_id)
        .expect("state zero hash");
    let checkpoint = ledger
        .checkpoint(&terms, coin_id, 1, previous)
        .expect("checkpoint");
    OfficialState {
        hub_state_signature: sign_hash(
            &key(2),
            &checkpoint.hub_state_hash(&terms).expect("state hash"),
        ),
        checkpoint,
    }
}

fn signed_result(status: ReservationStatus, request_id: u8) -> SignedReservationResult {
    let result = ReservationResult {
        network_id: terms().network_id,
        request_id: [request_id; 32],
        funding_coin_id: [0x42; 32],
        reservation_nonce: [0x55; 32],
        authorization_hash: [0x66; 32],
        status,
        state_sequence: None,
        checkpoint_hash: None,
        observed_peak_height: 150,
        acceptance_cutoff_height: 200,
        scheduled_close_height: 210,
        ledger_written: false,
    };
    SignedReservationResult {
        hub_result_signature: sign_hash(&key(2), &result.hash().expect("result hash")),
        result,
    }
}

#[test]
fn double_sign_evidence_is_canonical_verifiable_and_round_trips() {
    let first = signed_state(Ledger { entries: vec![] });
    let entry = LedgerEntry {
        merchant_puzzle_hash: [0x10; 32],
        merchant_receipt_public_key: public_key_bytes(&key(3)),
        amount: 100,
        reservation_nonce: [0x20; 32],
    };
    let second = signed_state(Ledger {
        entries: vec![entry],
    });
    let evidence = DoubleSignEvidence::new(&terms(), second, first).expect("evidence");
    evidence.validate(&terms()).expect("validate evidence");
    evidence.hash(&terms()).expect("evidence hash");
    let decoded = DoubleSignEvidence::from_canonical_bytes(&evidence.canonical_bytes())
        .expect("evidence decode");
    assert_eq!(decoded, evidence);
    decoded.validate(&terms()).expect("decoded evidence");
    assert_eq!(
        DoubleSignEvidence::new(&terms(), evidence.first.clone(), evidence.first.clone()),
        Err(ProtocolError::EvidenceNotConflicting)
    );
}

#[test]
fn conflicting_result_evidence_is_canonical_verifiable_and_round_trips() {
    let first = signed_result(ReservationStatus::RejectedFreezing, 1);
    let second = signed_result(ReservationStatus::InvalidAuthorization, 2);
    let evidence = ConflictingResultEvidence::new(&terms(), second, first).expect("evidence");
    evidence.validate(&terms()).expect("validate evidence");
    evidence.hash(&terms()).expect("evidence hash");
    let decoded = ConflictingResultEvidence::from_canonical_bytes(&evidence.canonical_bytes())
        .expect("evidence decode");
    assert_eq!(decoded, evidence);
    decoded.validate(&terms()).expect("decoded evidence");
    assert_eq!(
        ConflictingResultEvidence::new(&terms(), evidence.first.clone(), evidence.first.clone()),
        Err(ProtocolError::EvidenceNotConflicting)
    );
}
