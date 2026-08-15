use std::{process::Command, sync::Arc, thread};

use chia_bls::SecretKey;
use rusqlite::{Connection, params};
use tempfile::tempdir;
use xhub_protocol_v3_6::{
    CanonicalEncode, ChannelTerms, Ledger, LedgerEntry, OfficialState, RecoveryPackage, StateZero,
    public_key_bytes, sign_hash,
};
use xhub_watchtower_v3_6::{WatchtowerError, WatchtowerStore};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

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

fn entry(nonce: u8, merchant_key: &SecretKey) -> LedgerEntry {
    LedgerEntry {
        merchant_puzzle_hash: [nonce.wrapping_add(0x20); 32],
        merchant_receipt_public_key: public_key_bytes(merchant_key),
        amount: 100,
        reservation_nonce: [nonce; 32],
    }
}

fn package(entries: Vec<LedgerEntry>, sequence: u64, previous_hash: [u8; 32]) -> RecoveryPackage {
    let terms = terms();
    let signatures = entries
        .iter()
        .map(|entry| {
            sign_hash(
                &key(1),
                &entry
                    .authorization_hash(&terms, &FUNDING_COIN_ID)
                    .expect("authorization hash"),
            )
        })
        .collect();
    let checkpoint = Ledger {
        entries: entries.clone(),
    }
    .checkpoint(&terms, FUNDING_COIN_ID, sequence, previous_hash)
    .expect("checkpoint");
    let official_state = OfficialState {
        hub_state_signature: sign_hash(
            &key(2),
            &checkpoint.hub_state_hash(&terms).expect("state hash"),
        ),
        checkpoint,
    };
    RecoveryPackage {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_amount: terms.funding_amount,
        channel_terms: terms,
        official_state,
        entries,
        user_authorization_signatures: signatures,
    }
}

fn first_package(merchant_key: &SecretKey) -> RecoveryPackage {
    let terms = terms();
    let zero = StateZero::new(&terms)
        .expect("state zero")
        .hash(&terms, &FUNDING_COIN_ID)
        .expect("state zero hash");
    package(vec![entry(1, merchant_key)], 1, zero)
}

#[test]
fn accepts_complete_package_idempotently_and_quarantines_invalid_bytes() {
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    let bytes = package.canonical_bytes();
    let mut store = WatchtowerStore::open_in_memory().expect("store");

    let first = store.accept_package(&bytes, 1_000).expect("accepted");
    let repeated = store.accept_package(&bytes, 1_001).expect("idempotent");
    assert_eq!(first.checkpoint_hash, repeated.checkpoint_hash);
    assert_eq!(
        first.recovery_package_content_hash,
        repeated.recovery_package_content_hash
    );
    assert_eq!(
        store
            .latest_package(FUNDING_COIN_ID)
            .expect("latest package"),
        package
    );

    let truncated = &bytes[..bytes.len() - 1];
    assert!(matches!(
        store.accept_package(truncated, 1_002),
        Err(WatchtowerError::Protocol(_))
    ));
    let quarantine = store.quarantined().expect("quarantine");
    assert_eq!(quarantine.len(), 1);
    assert_eq!(quarantine[0].reason_code, "INVALID_ENCODING");

    let mut invalid_reveal = package.clone();
    invalid_reveal.funding_puzzle_reveal = vec![0xff];
    assert!(matches!(
        store.accept_package(&invalid_reveal.canonical_bytes(), 1_003),
        Err(WatchtowerError::Invalid(_))
    ));
    let quarantine = store.quarantined().expect("quarantine");
    assert_eq!(quarantine.len(), 2);
    assert_eq!(quarantine[1].reason_code, "INVALID_FUNDING_PUZZLE");
}

#[test]
fn same_sequence_conflicts_and_append_only_mutations_never_replace_the_head() {
    let merchant_key = key(3);
    let first = first_package(&merchant_key);
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    store
        .accept_package(&first.canonical_bytes(), 1_000)
        .expect("first");

    let terms = terms();
    let zero = StateZero::new(&terms)
        .expect("zero")
        .hash(&terms, &FUNDING_COIN_ID)
        .expect("zero hash");
    let conflict = package(vec![entry(9, &merchant_key)], 1, zero);
    assert!(matches!(
        store.accept_package(&conflict.canonical_bytes(), 1_001),
        Err(WatchtowerError::StateConflict)
    ));

    let first_hash = first
        .official_state
        .checkpoint
        .hash(&first.channel_terms)
        .expect("first hash");
    let mutation = package(
        vec![entry(8, &merchant_key), entry(2, &merchant_key)],
        2,
        first_hash,
    );
    assert!(matches!(
        store.accept_package(&mutation.canonical_bytes(), 1_002),
        Err(WatchtowerError::StateConflict)
    ));
    assert_eq!(
        store
            .latest_package(FUNDING_COIN_ID)
            .expect("latest package"),
        first
    );
    assert_eq!(store.quarantined().expect("quarantine").len(), 2);
}

#[test]
fn merchant_confirmation_requires_persisted_package_and_duplicate_key_ids_do_not_stack() {
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    for (signer, domain) in [
        ("wt-1", "domain-a"),
        ("wt-2", "domain-b"),
        ("wt-3", "domain-c"),
    ] {
        store
            .register_confirmer(signer, domain, public_key_bytes(&merchant_key), 900)
            .expect("register confirmer");
    }
    assert!(matches!(
        store.sign_confirmation(FUNDING_COIN_ID, 1, 0, "wt-1", &merchant_key),
        Err(WatchtowerError::PackageNotFound)
    ));

    store
        .accept_package(&package.canonical_bytes(), 1_000)
        .expect("accepted");
    let one = store
        .sign_confirmation(FUNDING_COIN_ID, 1, 0, "wt-1", &merchant_key)
        .expect("first confirmation");
    store.record_confirmation(&one, 1_001).expect("record one");
    store
        .record_confirmation(&one, 1_002)
        .expect("idempotent record");
    assert!(
        store
            .greenlight_status(FUNDING_COIN_ID, 1, 0, 1)
            .expect("1-of-3")
            .delivered
    );
    assert!(
        !store
            .greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
            .expect("2-of-3")
            .delivered
    );

    for (index, signer) in ["wt-2", "wt-3"].into_iter().enumerate() {
        let confirmation = store
            .sign_confirmation(FUNDING_COIN_ID, 1, 0, signer, &merchant_key)
            .expect("confirmation");
        store
            .record_confirmation(&confirmation, 1_003 + index as u64)
            .expect("record confirmation");
    }
    let status = store
        .greenlight_status(FUNDING_COIN_ID, 1, 0, 3)
        .expect("3-of-3");
    assert!(!status.delivered);
    assert_eq!((status.signer_count, status.failure_domain_count), (1, 1));
}

#[test]
fn same_failure_domain_does_not_satisfy_two_domain_threshold() {
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    store
        .accept_package(&package.canonical_bytes(), 1_000)
        .expect("accepted");
    for signer in ["wt-1", "wt-2"] {
        store
            .register_confirmer(signer, "same-vps", public_key_bytes(&merchant_key), 900)
            .expect("register");
        let confirmation = store
            .sign_confirmation(FUNDING_COIN_ID, 1, 0, signer, &merchant_key)
            .expect("sign");
        store
            .record_confirmation(&confirmation, 1_001)
            .expect("record");
    }
    let status = store
        .greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
        .expect("status");
    assert_eq!((status.signer_count, status.failure_domain_count), (1, 1));
    assert!(!status.delivered);
}

#[test]
fn production_greenlight_requires_merchant_receipt_and_independent_custody_domains() {
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    store
        .accept_package(&package.canonical_bytes(), 1_000)
        .expect("accepted");
    store
        .register_confirmer(
            "merchant-receipt",
            "merchant-domain",
            public_key_bytes(&merchant_key),
            900,
        )
        .expect("register merchant");
    for (id, domain, signer_key) in [
        ("custody-1", "domain-a", key(4)),
        ("custody-2", "domain-b", key(5)),
        ("custody-3", "domain-a", key(6)),
    ] {
        store
            .register_custody_attester(id, domain, public_key_bytes(&signer_key), 900)
            .expect("register custody attester");
    }

    assert!(matches!(
        store.custody_attestation(FUNDING_COIN_ID, 1, 0),
        Err(WatchtowerError::MerchantConfirmationRequired)
    ));
    let merchant = store
        .sign_confirmation(FUNDING_COIN_ID, 1, 0, "merchant-receipt", &merchant_key)
        .expect("merchant confirmation");
    store
        .record_confirmation(&merchant, 1_001)
        .expect("record merchant confirmation");

    let one = store
        .sign_custody_attestation(FUNDING_COIN_ID, 1, 0, "custody-1", &key(4))
        .expect("custody one");
    store
        .record_custody_attestation(&one, 1_002)
        .expect("record custody one");
    let partial = store
        .production_greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
        .expect("partial status");
    assert!(partial.merchant_delivered);
    assert_eq!(
        (
            partial.custody_attester_count,
            partial.custody_failure_domain_count
        ),
        (1, 1)
    );
    assert!(!partial.production_ready);

    let same_domain = store
        .sign_custody_attestation(FUNDING_COIN_ID, 1, 0, "custody-3", &key(6))
        .expect("same domain custody");
    store
        .record_custody_attestation(&same_domain, 1_003)
        .expect("record same domain custody");
    let same_domain_status = store
        .production_greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
        .expect("same domain status");
    assert_eq!(
        (
            same_domain_status.custody_attester_count,
            same_domain_status.custody_failure_domain_count,
        ),
        (2, 1)
    );
    assert!(!same_domain_status.production_ready);
    let single_vps = store
        .single_vps_test_greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
        .expect("single VPS test status");
    assert_eq!(
        (
            single_vps.custody_attester_count,
            single_vps.observed_failure_domain_count,
        ),
        (2, 1)
    );
    assert!(single_vps.test_ready);

    let independent = store
        .sign_custody_attestation(FUNDING_COIN_ID, 1, 0, "custody-2", &key(5))
        .expect("independent custody");
    store
        .record_custody_attestation(&independent, 1_004)
        .expect("record independent custody");
    let ready = store
        .production_greenlight_status(FUNDING_COIN_ID, 1, 0, 2)
        .expect("ready status");
    assert_eq!(
        (
            ready.custody_attester_count,
            ready.custody_failure_domain_count
        ),
        (3, 2)
    );
    assert!(ready.production_ready);
}

#[test]
fn custody_attester_public_key_cannot_be_registered_under_two_identities() {
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    let shared = public_key_bytes(&key(4));
    store
        .register_custody_attester("custody-1", "domain-a", shared, 1)
        .expect("first identity");
    assert!(matches!(
        store.register_custody_attester("custody-2", "domain-b", shared, 2),
        Err(WatchtowerError::AttesterConflict)
    ));
}

#[test]
fn concurrent_store_instances_preserve_a_single_append_only_head() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let merchant_key = key(3);
    let first = first_package(&merchant_key);
    let first_hash = first
        .official_state
        .checkpoint
        .hash(&first.channel_terms)
        .expect("first checkpoint hash");
    let latest = package(
        vec![entry(1, &merchant_key), entry(2, &merchant_key)],
        2,
        first_hash,
    );
    {
        let mut store = WatchtowerStore::open(&path).expect("initial store");
        store
            .accept_package(&first.canonical_bytes(), 1_000)
            .expect("first package");
    }

    let shared_path = Arc::new(path.clone());
    let latest_bytes = Arc::new(latest.canonical_bytes());
    let workers = (0..8)
        .map(|index| {
            let path = Arc::clone(&shared_path);
            let bytes = Arc::clone(&latest_bytes);
            thread::spawn(move || {
                let mut store = WatchtowerStore::open(path.as_path()).expect("worker store");
                store
                    .accept_package(&bytes, 1_001 + index)
                    .expect("concurrent package append");
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("worker join");
    }

    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert_eq!(
        store.latest_package(FUNDING_COIN_ID).expect("latest"),
        latest
    );
    assert_eq!(store.quarantined().expect("quarantine").len(), 0);
    assert_eq!(store.durability_mode().expect("durability").0, "wal");
}

#[test]
fn wal_restart_preserves_packages_quarantine_and_durability_settings() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    let bytes = package.canonical_bytes();
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store.accept_package(&bytes, 1_000).expect("package");
        assert!(
            store
                .accept_package(&bytes[..bytes.len() - 1], 1_001)
                .is_err()
        );
        let (journal_mode, synchronous) = store.durability_mode().expect("durability");
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
    }

    let store = WatchtowerStore::open(&path).expect("restarted store");
    assert_eq!(
        store.latest_package(FUNDING_COIN_ID).expect("latest"),
        package
    );
    let quarantined = store.quarantined().expect("quarantine");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].reason_code, "INVALID_ENCODING");
    assert_eq!(
        store.durability_mode().expect("durability"),
        ("wal".into(), 2)
    );
}

#[test]
fn wal_recovery_discards_an_aborted_uncommitted_transaction() {
    const CRASH_DB_ENV: &str = "XHUB_V36_WATCHTOWER_CRASH_DB";
    if let Some(path) = std::env::var_os(CRASH_DB_ENV) {
        let connection = Connection::open(path).expect("crash child database");
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 PRAGMA cache_size = 10;
                 PRAGMA cache_spill = ON;
                 BEGIN IMMEDIATE;",
            )
            .expect("begin crash transaction");
        let payload = vec![0x5a_u8; 4_096];
        for index in 0..256_u64 {
            let mut content_hash = [0_u8; 32];
            content_hash[..8].copy_from_slice(&index.to_be_bytes());
            connection
                .execute(
                    "INSERT INTO v36_watchtower_quarantine (
                       content_hash, package_blob, reason_code, reason, received_at
                     ) VALUES (?1, ?2, 'CRASH_INJECTION', 'must roll back', ?3)",
                    params![content_hash.as_slice(), &payload, index as i64],
                )
                .expect("uncommitted crash row");
        }
        std::process::abort();
    }

    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let merchant_key = key(3);
    let package = first_package(&merchant_key);
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&package.canonical_bytes(), 1_000)
            .expect("committed package");
    }

    let status = Command::new(std::env::current_exe().expect("test executable"))
        .arg("--exact")
        .arg("wal_recovery_discards_an_aborted_uncommitted_transaction")
        .env(CRASH_DB_ENV, &path)
        .status()
        .expect("crash child process");
    assert!(!status.success());

    let store = WatchtowerStore::open(&path).expect("recovered store");
    assert_eq!(
        store.latest_package(FUNDING_COIN_ID).expect("latest"),
        package
    );
    assert!(store.quarantined().expect("quarantine").is_empty());
    drop(store);
    let connection = Connection::open(&path).expect("integrity database");
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity check");
    assert_eq!(integrity, "ok");
}
