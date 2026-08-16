use std::{path::Path, thread, time::Duration};

use chia_bls::SecretKey;
use rusqlite::{Connection, params};
use tempfile::TempDir;
use xhub_hub_v3_6::{
    ChannelRegistration, FailurePoint, HubError, HubStore, ReservationLookup, ReservationRequest,
    StateCandidate,
};
use xhub_protocol_v3_6::{
    ChannelTerms, Ledger, LedgerEntry, ReservationStatus, StateZero, public_key_bytes, sign_hash,
};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn registration() -> ChannelRegistration {
    let user_key = key(1);
    let hub_key = key(2);
    let terms = ChannelTerms::new(
        [0xaa; 32],
        100,
        10,
        50,
        public_key_bytes(&user_key),
        public_key_bytes(&hub_key),
        [0x36; 32],
        1_000,
        [0x77; 32],
    )
    .expect("channel terms");
    ChannelRegistration {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_birth_height: 100,
        channel_terms: terms,
    }
}

fn request(nonce: u8, amount: u64) -> ReservationRequest {
    let registration = registration();
    let entry = LedgerEntry {
        merchant_puzzle_hash: [nonce.wrapping_add(0x20); 32],
        merchant_receipt_public_key: public_key_bytes(&key(nonce.wrapping_add(10))),
        amount,
        reservation_nonce: [nonce; 32],
    };
    let authorization_hash = entry
        .authorization_hash(&registration.channel_terms, &FUNDING_COIN_ID)
        .expect("authorization hash");
    ReservationRequest {
        request_id: [nonce.wrapping_add(0x80); 32],
        funding_coin_id: FUNDING_COIN_ID,
        ledger_entry: entry,
        user_authorization_signature: sign_hash(&key(1), &authorization_hash),
    }
}

fn initialized_memory_store() -> HubStore {
    let mut store = HubStore::open_in_memory().expect("store");
    store
        .register_channel(&registration(), 1_000)
        .expect("register channel");
    store
}

fn initialized_file_store(path: &Path) -> HubStore {
    let mut store = HubStore::open(path).expect("store");
    store
        .register_channel(&registration(), 1_000)
        .expect("register channel");
    store
}

#[test]
fn first_reservation_advances_from_state_zero_and_is_exactly_idempotent() {
    let mut store = initialized_memory_store();
    let request = request(1, 100);
    let first = store
        .reserve(&request, 150, &key(2), 1_001)
        .expect("first reservation");
    let package = first.recovery_package.as_ref().expect("recovery package");
    package.validate().expect("package validation");
    assert_eq!(first.signed_result.result.status, ReservationStatus::Signed);
    assert_eq!(first.signed_result.result.state_sequence, Some(1));
    assert!(first.signed_result.result.ledger_written);
    assert_eq!(package.entries, vec![request.ledger_entry.clone()]);

    let state_zero_hash = StateZero::new(&registration().channel_terms)
        .expect("state zero")
        .hash(&registration().channel_terms, &FUNDING_COIN_ID)
        .expect("state zero hash");
    assert_eq!(
        package.official_state.checkpoint.previous_checkpoint_hash,
        state_zero_hash
    );

    let retried = store
        .reserve(&request, 151, &key(2), 1_002)
        .expect("idempotent retry");
    assert_eq!(retried, first);
    assert_eq!(store.intent_count(FUNDING_COIN_ID).expect("intents"), 1);
    let snapshot = store.channel_snapshot(FUNDING_COIN_ID).expect("snapshot");
    assert_eq!((snapshot.latest_sequence, snapshot.entry_count), (1, 1));
}

#[test]
fn append_only_states_are_adjacent_and_mutations_or_jumps_are_rejected() {
    let mut store = initialized_memory_store();
    let first = store
        .reserve(&request(1, 100), 150, &key(2), 1_001)
        .expect("first");
    let second = store
        .reserve(&request(2, 200), 151, &key(2), 1_002)
        .expect("second");
    let first_package = first.recovery_package.expect("first package");
    let second_package = second.recovery_package.expect("second package");
    assert_eq!(
        &second_package.entries[..first_package.entries.len()],
        first_package.entries
    );
    assert_eq!(
        second_package
            .official_state
            .checkpoint
            .previous_checkpoint_hash,
        first_package
            .official_state
            .checkpoint
            .hash(&registration().channel_terms)
            .expect("first hash")
    );

    let third = request(3, 50);
    let mut entries = second_package.entries.clone();
    let mut signatures = second_package.user_authorization_signatures.clone();
    entries.push(third.ledger_entry.clone());
    signatures.push(third.user_authorization_signature);
    let previous = second_package
        .official_state
        .checkpoint
        .hash(&registration().channel_terms)
        .expect("second hash");
    let checkpoint = Ledger {
        entries: entries.clone(),
    }
    .checkpoint(&registration().channel_terms, FUNDING_COIN_ID, 3, previous)
    .expect("third checkpoint");
    let valid = StateCandidate {
        checkpoint: checkpoint.clone(),
        entries: entries.clone(),
        user_authorization_signatures: signatures.clone(),
    };
    store
        .validate_next_state(FUNDING_COIN_ID, &valid)
        .expect("valid adjacent candidate");

    let mut jump = valid.clone();
    jump.checkpoint.state_sequence = 4;
    assert!(matches!(
        store.validate_next_state(FUNDING_COIN_ID, &jump),
        Err(HubError::StateConflict)
    ));

    let mut wrong_previous = valid.clone();
    wrong_previous.checkpoint.previous_checkpoint_hash = [0x99; 32];
    assert!(matches!(
        store.validate_next_state(FUNDING_COIN_ID, &wrong_previous),
        Err(HubError::StateConflict)
    ));

    let mut mutated = valid;
    mutated.entries[0].amount += 1;
    mutated.checkpoint = Ledger {
        entries: mutated.entries.clone(),
    }
    .checkpoint(&registration().channel_terms, FUNDING_COIN_ID, 3, previous)
    .expect("mutated checkpoint");
    assert!(matches!(
        store.validate_next_state(FUNDING_COIN_ID, &mutated),
        Err(HubError::StateConflict)
    ));
}

#[test]
fn nonce_conflict_never_changes_the_signed_ledger() {
    let mut store = initialized_memory_store();
    let accepted = request(7, 100);
    store
        .reserve(&accepted, 150, &key(2), 1_001)
        .expect("accepted");
    let conflicting = request(7, 101);
    assert!(matches!(
        store.reserve(&conflicting, 151, &key(2), 1_002),
        Err(HubError::NonceConflict)
    ));
    let snapshot = store.channel_snapshot(FUNDING_COIN_ID).expect("snapshot");
    assert_eq!((snapshot.latest_sequence, snapshot.entry_count), (1, 1));
}

#[test]
fn deterministic_rejections_are_signed_persisted_and_do_not_write_the_ledger() {
    let mut store = initialized_memory_store();
    let freezing = request(8, 100);
    let rejected = store
        .reserve(&freezing, 200, &key(2), 1_001)
        .expect("freezing rejection");
    assert_eq!(
        rejected.signed_result.result.status,
        ReservationStatus::RejectedFreezing
    );
    assert!(!rejected.signed_result.result.ledger_written);
    assert!(rejected.recovery_package.is_none());
    assert_eq!(
        store
            .channel_snapshot(FUNDING_COIN_ID)
            .expect("snapshot")
            .latest_sequence,
        0
    );
    assert_eq!(
        store
            .reserve(&freezing, 199, &key(2), 1_002)
            .expect("same persisted rejection"),
        rejected
    );

    let mut invalid = request(9, 100);
    let auth_hash = invalid
        .authorization_hash(&registration().channel_terms)
        .expect("auth hash");
    invalid.user_authorization_signature = sign_hash(&key(99), &auth_hash);
    let rejected = store
        .reserve(&invalid, 150, &key(2), 1_003)
        .expect("authorization rejection");
    assert_eq!(
        rejected.signed_result.result.status,
        ReservationStatus::InvalidAuthorization
    );
    assert!(!rejected.signed_result.result.ledger_written);
}

fn crash_recovery(failure_point: FailurePoint) {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("hub.sqlite3");
    let request = request(11, 125);
    {
        let mut store = initialized_file_store(&path);
        assert!(matches!(
            store.reserve_with_failure(&request, 150, &key(2), 1_001, failure_point),
            Err(HubError::InjectedFailure(point)) if point == failure_point
        ));
        assert_eq!(
            store
                .channel_snapshot(FUNDING_COIN_ID)
                .expect("snapshot")
                .latest_sequence,
            0
        );
        assert_eq!(
            store
                .reservation_status(FUNDING_COIN_ID, request.ledger_entry.reservation_nonce)
                .expect("pending status"),
            ReservationLookup::Pending
        );
    }

    let mut reopened = HubStore::open(&path).expect("reopen");
    let (journal, synchronous) = reopened.durability_mode().expect("durability mode");
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 2);
    let recovered = reopened
        .recover_pending(&key(2), 1_002)
        .expect("recover pending");
    assert_eq!(recovered.len(), 1);
    recovered[0]
        .recovery_package
        .as_ref()
        .expect("recovery package")
        .validate()
        .expect("recovery package validation");
    assert_eq!(
        reopened
            .channel_snapshot(FUNDING_COIN_ID)
            .expect("snapshot")
            .latest_sequence,
        1
    );
    assert_eq!(reopened.intent_count(FUNDING_COIN_ID).expect("intent"), 1);
    assert_eq!(
        reopened
            .reserve(&request, 151, &key(2), 1_003)
            .expect("retry after recovery"),
        recovered[0]
    );
}

#[test]
fn crash_after_preparation_commit_recovers_without_double_signing() {
    crash_recovery(FailurePoint::AfterPreparationCommit);
}

#[test]
fn crash_after_state_signature_recovers_without_double_signing() {
    crash_recovery(FailurePoint::AfterStateSignature);
}

#[test]
fn concurrent_writers_are_serialized_into_unique_adjacent_sequences() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("hub.sqlite3");
    initialized_file_store(&path);
    let handles = (1_u8..=6)
        .map(|nonce| {
            let path = path.clone();
            thread::spawn(move || {
                let request = request(nonce, 100);
                for attempt in 0..100_u64 {
                    let mut store = HubStore::open(&path).expect("writer store");
                    match store.reserve(&request, 150, &key(2), 2_000 + attempt) {
                        Ok(outcome) => return outcome.signed_result.result.state_sequence.unwrap(),
                        Err(HubError::PendingTransition) => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("unexpected reservation error: {error}"),
                    }
                }
                panic!("writer did not make progress")
            })
        })
        .collect::<Vec<_>>();
    let mut sequences = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect::<Vec<_>>();
    sequences.sort_unstable();
    assert_eq!(sequences, vec![1, 2, 3, 4, 5, 6]);
    let store = HubStore::open(&path).expect("final store");
    let snapshot = store.channel_snapshot(FUNDING_COIN_ID).expect("snapshot");
    assert_eq!((snapshot.latest_sequence, snapshot.entry_count), (6, 6));
    store
        .latest_recovery_package(FUNDING_COIN_ID)
        .expect("latest package")
        .expect("package")
        .validate()
        .expect("package validation");
}

#[test]
fn persisted_index_or_request_tampering_is_detected_on_read() {
    let temporary = TempDir::new().expect("temporary directory");
    let request_path = temporary.path().join("request-tamper.sqlite3");
    let request_12 = request(12, 100);
    {
        let mut store = initialized_file_store(&request_path);
        store
            .reserve(&request_12, 150, &key(2), 1_001)
            .expect("reservation");
    }
    Connection::open(&request_path)
        .expect("tamper connection")
        .execute(
            "UPDATE v36_reservations SET request_fingerprint = zeroblob(32)
             WHERE funding_coin_id = ?1 AND reservation_nonce = ?2",
            params![
                FUNDING_COIN_ID.as_slice(),
                request_12.ledger_entry.reservation_nonce.as_slice()
            ],
        )
        .expect("tamper request fingerprint");
    let store = HubStore::open(&request_path).expect("reopen request store");
    assert!(matches!(
        store.reservation_status(FUNDING_COIN_ID, request_12.ledger_entry.reservation_nonce),
        Err(HubError::Corrupt(_))
    ));

    let pointer_path = temporary.path().join("pointer-tamper.sqlite3");
    {
        let mut store = initialized_file_store(&pointer_path);
        store
            .reserve(&request(13, 100), 150, &key(2), 1_001)
            .expect("reservation");
    }
    Connection::open(&pointer_path)
        .expect("tamper connection")
        .execute(
            "UPDATE v36_channels SET latest_checkpoint_hash = zeroblob(32)
             WHERE funding_coin_id = ?1",
            [FUNDING_COIN_ID.as_slice()],
        )
        .expect("tamper latest pointer");
    let store = HubStore::open(&pointer_path).expect("reopen pointer store");
    assert!(matches!(
        store.channel_snapshot(FUNDING_COIN_ID),
        Err(HubError::Corrupt(_))
    ));
}
