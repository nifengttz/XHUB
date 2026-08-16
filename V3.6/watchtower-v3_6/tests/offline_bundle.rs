use chia_bls::SecretKey;
use chia_protocol::Coin;
use xhub_protocol_v3_6::{
    ChannelTerms, Ledger, LedgerEntry, OfficialState, RecoveryPackage, StateZero, public_key_bytes,
    sign_hash, state_rules_hash,
};
use xhub_puzzles_v3_6::{
    ClosingCoinKind, challenge_spend_material, funding_puzzle_reveal, module_hashes,
    state_zero_challenge_spend_material,
};
use xhub_watchtower_v3_6::bundle::{ChainSnapshot, build_offline_challenge_bundle, test_fee_coin};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];
const BIRTH_HEIGHT: u64 = 1_000;
const DEADLINE: u64 = 1_050;

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn terms() -> ChannelTerms {
    let hashes = module_hashes();
    ChannelTerms::new(
        [0xaa; 32],
        100,
        10,
        50,
        public_key_bytes(&key(1)),
        public_key_bytes(&key(2)),
        state_rules_hash(
            &hashes.initial_closing,
            &hashes.subsequent_closing,
            &hashes.merchant_payment,
        ),
        1_000,
        [0x77; 32],
    )
    .expect("terms")
}

fn package(count: usize, sequence: u64, previous: [u8; 32]) -> RecoveryPackage {
    let terms = terms();
    let entries = (0..count)
        .map(|index| LedgerEntry {
            merchant_puzzle_hash: [0x20 + index as u8; 32],
            merchant_receipt_public_key: public_key_bytes(&key(3)),
            amount: 100,
            reservation_nonce: [index as u8 + 1; 32],
        })
        .collect::<Vec<_>>();
    let checkpoint = Ledger {
        entries: entries.clone(),
    }
    .checkpoint(&terms, FUNDING_COIN_ID, sequence, previous)
    .expect("checkpoint");
    let (_, reveal) = funding_puzzle_reveal(&terms).expect("Funding reveal");
    RecoveryPackage {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: reveal.to_vec(),
        funding_amount: terms.funding_amount,
        channel_terms: terms.clone(),
        official_state: OfficialState {
            hub_state_signature: sign_hash(
                &key(2),
                &checkpoint.hub_state_hash(&terms).expect("Hub hash"),
            ),
            checkpoint,
        },
        user_authorization_signatures: entries
            .iter()
            .map(|entry| {
                sign_hash(
                    &key(1),
                    &entry
                        .authorization_hash(&terms, &FUNDING_COIN_ID)
                        .expect("authorization hash"),
                )
            })
            .collect(),
        entries,
    }
}

fn packages() -> (RecoveryPackage, RecoveryPackage) {
    let terms = terms();
    let zero = StateZero::new(&terms)
        .expect("State 0")
        .hash(&terms, &FUNDING_COIN_ID)
        .expect("State 0 hash");
    let current = package(1, 1, zero);
    let previous = current
        .official_state
        .checkpoint
        .hash(&terms)
        .expect("current hash");
    (current, package(2, 2, previous))
}

fn snapshot(coin: Coin, peak_height: u64) -> ChainSnapshot {
    ChainSnapshot {
        peak_height,
        peak_header_hash: [0x99; 32],
        closing_coin_id: coin.coin_id().to_bytes(),
        closing_coin: coin,
        closing_birth_height: BIRTH_HEIGHT,
        closing_spent_height: None,
    }
}

fn fee() -> xhub_watchtower_v3_6::bundle::TestFeeSponsor {
    test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee Coin")
}

#[test]
fn builds_state_zero_initial_and_subsequent_offline_bundles() {
    let (current, latest) = packages();
    let zero_material =
        state_zero_challenge_spend_material(&latest, BIRTH_HEIGHT, DEADLINE).expect("State 0");
    let zero_coin = Coin::new(
        FUNDING_COIN_ID.into(),
        zero_material.expected_closing_puzzle_hash.into(),
        latest.funding_amount,
    );
    let zero = build_offline_challenge_bundle(
        None,
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
        snapshot(zero_coin, DEADLINE - 1),
        &fee(),
    )
    .expect("State 0 bundle");
    assert_eq!(zero.report().fee_mojo, 2);
    assert_eq!(zero.report().removal_amount_mojo, 1_020);
    assert_eq!(zero.report().addition_amount_mojo, 1_018);
    assert!(zero.report().consensus_conditions_verified);
    assert!(zero.report().aggregate_signature_verified);
    assert!(zero.report().spend_bundle_created);
    assert_ne!(zero.commitment(), [0; 32]);
    assert!(!zero.report().broadcast_enabled);
    assert!(!zero.report().broadcast_ready);
    assert!(!zero.report().chain_broadcast);

    for kind in [ClosingCoinKind::Initial, ClosingCoinKind::Subsequent] {
        let material = challenge_spend_material(&current, &latest, kind, BIRTH_HEIGHT, DEADLINE)
            .expect("challenge material");
        let parent = match kind {
            ClosingCoinKind::Initial => FUNDING_COIN_ID,
            ClosingCoinKind::Subsequent => [0x55; 32],
        };
        let coin = Coin::new(
            parent.into(),
            material.expected_closing_puzzle_hash.into(),
            latest.funding_amount,
        );
        let bundle = build_offline_challenge_bundle(
            Some(&current),
            &latest,
            kind,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(coin, DEADLINE - 1),
            &fee(),
        )
        .expect("signed offline bundle");
        assert_ne!(bundle.commitment(), zero.commitment());
    }
}

#[test]
fn identical_material_has_a_stable_commitment_and_fee_material_changes_it() {
    let (current, latest) = packages();
    let material = challenge_spend_material(
        &current,
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
    )
    .expect("material");
    let coin = Coin::new(
        FUNDING_COIN_ID.into(),
        material.expected_closing_puzzle_hash.into(),
        latest.funding_amount,
    );
    let first = build_offline_challenge_bundle(
        Some(&current),
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
        snapshot(coin, DEADLINE - 1),
        &fee(),
    )
    .expect("first bundle");
    let repeated = build_offline_challenge_bundle(
        Some(&current),
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
        snapshot(coin, DEADLINE - 1),
        &fee(),
    )
    .expect("repeated bundle");
    assert_eq!(first.commitment(), repeated.commitment());

    let different_fee =
        test_fee_coin([0xf2; 32], 20, key(8), [0xf3; 32], 2).expect("different fee Coin");
    let changed = build_offline_challenge_bundle(
        Some(&current),
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
        snapshot(coin, DEADLINE - 1),
        &different_fee,
    )
    .expect("changed fee bundle");
    assert_ne!(first.commitment(), changed.commitment());
}

#[test]
fn rejects_deadline_coin_and_fee_tampering() {
    let (current, latest) = packages();
    let material = challenge_spend_material(
        &current,
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
    )
    .expect("material");
    let coin = Coin::new(
        FUNDING_COIN_ID.into(),
        material.expected_closing_puzzle_hash.into(),
        latest.funding_amount,
    );
    assert!(
        build_offline_challenge_bundle(
            Some(&current),
            &latest,
            ClosingCoinKind::Initial,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(coin, DEADLINE),
            &fee(),
        )
        .is_err()
    );

    let wrong_amount_coin = Coin::new(
        FUNDING_COIN_ID.into(),
        material.expected_closing_puzzle_hash.into(),
        latest.funding_amount - 1,
    );
    assert!(
        build_offline_challenge_bundle(
            Some(&current),
            &latest,
            ClosingCoinKind::Initial,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(wrong_amount_coin, DEADLINE - 1),
            &fee(),
        )
        .is_err()
    );

    let wrong_coin = Coin::new(
        FUNDING_COIN_ID.into(),
        [0xee; 32].into(),
        latest.funding_amount,
    );
    assert!(
        build_offline_challenge_bundle(
            Some(&current),
            &latest,
            ClosingCoinKind::Initial,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(wrong_coin, DEADLINE - 1),
            &fee(),
        )
        .is_err()
    );

    let bad_fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 20).expect("bad fee vector");
    assert!(
        build_offline_challenge_bundle(
            Some(&current),
            &latest,
            ClosingCoinKind::Initial,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(coin, DEADLINE - 1),
            &bad_fee,
        )
        .is_err()
    );

    let mut wrong_signature = latest.clone();
    wrong_signature.official_state.hub_state_signature = current.official_state.hub_state_signature;
    assert!(
        build_offline_challenge_bundle(
            Some(&current),
            &wrong_signature,
            ClosingCoinKind::Initial,
            BIRTH_HEIGHT,
            DEADLINE,
            snapshot(coin, DEADLINE - 1),
            &fee(),
        )
        .is_err()
    );
}

#[test]
fn rejects_reorg_or_spend_between_construction_and_broadcast_check() {
    let (current, latest) = packages();
    let material = challenge_spend_material(
        &current,
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
    )
    .expect("material");
    let coin = Coin::new(
        FUNDING_COIN_ID.into(),
        material.expected_closing_puzzle_hash.into(),
        latest.funding_amount,
    );
    let original = snapshot(coin, DEADLINE - 2);
    let bundle = build_offline_challenge_bundle(
        Some(&current),
        &latest,
        ClosingCoinKind::Initial,
        BIRTH_HEIGHT,
        DEADLINE,
        original.clone(),
        &fee(),
    )
    .expect("offline bundle");
    bundle
        .validate_pre_broadcast_snapshot(&original)
        .expect("unchanged snapshot");

    let mut reorg = original.clone();
    reorg.peak_header_hash = [0x98; 32];
    assert!(bundle.validate_pre_broadcast_snapshot(&reorg).is_err());
    let mut spent = original;
    spent.closing_spent_height = Some(DEADLINE - 1);
    assert!(bundle.validate_pre_broadcast_snapshot(&spent).is_err());
}
