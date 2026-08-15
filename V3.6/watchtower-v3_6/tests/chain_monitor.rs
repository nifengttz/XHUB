use std::{fs, path::PathBuf};

use chia_bls::SecretKey;
use chia_protocol::Coin;
use chia_protocol::{Bytes, Bytes32 as ChiaBytes32};
use clvm_traits::ToClvm;
use clvm_utils::tree_hash;
use clvmr::{
    Allocator,
    serde::{node_from_bytes, node_to_bytes},
};
use rusqlite::Connection;
use tempfile::tempdir;
use xhub_protocol_v3_6::{
    CanonicalEncode, ChannelTerms, Ledger, LedgerEntry, OfficialState, RecoveryPackage, StateZero,
    closing_state_hash, one_arg_puzzle_hash, public_key_bytes, sign_hash, state_rules_hash,
};
use xhub_puzzles_v3_6::{ClosingCoinKind, funding_puzzle_reveal, module_hashes};
use xhub_watchtower_v3_6::backup::backup_replicas_are_consistent;
use xhub_watchtower_v3_6::backup::{
    BACKUP_HANDOFF_REJECTED, BACKUP_HANDOFF_VERIFIED, BACKUP_RESTORE_DRILL_PASSED,
    BackupKeyProvider, BackupRetentionPolicy, ENCRYPTED_BACKUP_DOMAIN, EncryptedBackupArtifact,
    database_backup_id, decode_encrypted_backup_artifact, encode_encrypted_backup_artifact,
    encrypted_backup_replicas_are_consistent, verified_backup_handoffs_are_consistent,
};
use zeroize::Zeroizing;

struct TestBackupKeys {
    key_id: [u8; 32],
    key: [u8; 32],
}

struct RejectingBackupKeys;

impl BackupKeyProvider for RejectingBackupKeys {
    fn load_backup_key(
        &self,
        _key_id: [u8; 32],
    ) -> xhub_watchtower_v3_6::Result<Zeroizing<[u8; 32]>> {
        Err(xhub_watchtower_v3_6::WatchtowerError::Invalid(
            "backup key unavailable".into(),
        ))
    }
}

impl BackupKeyProvider for TestBackupKeys {
    fn load_backup_key(
        &self,
        key_id: [u8; 32],
    ) -> xhub_watchtower_v3_6::Result<Zeroizing<[u8; 32]>> {
        if key_id != self.key_id {
            return Err(xhub_watchtower_v3_6::WatchtowerError::Invalid(
                "unknown backup key id".into(),
            ));
        }
        Ok(Zeroizing::new(self.key))
    }
}
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    approval::{
        APPROVAL_REVOKED_CHAIN_CHANGE, ApprovalStatement, DUAL_APPROVED_RECHECK_REQUIRED,
        PARTIALLY_APPROVED, SignedApproval,
    },
    authorization::{
        EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY, EXECUTION_AUTHORIZATION_EXPIRED,
        EXECUTION_AUTHORIZATION_INVALIDATED, EXECUTION_AUTHORIZATION_SUPERSEDED,
        EXECUTION_AUTHORIZED_SIMULATED_ONLY,
    },
    bundle::test_fee_coin,
    final_recheck::{
        FINAL_RECHECK_EXPIRED, FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE,
        FINAL_RECHECK_VERIFIED_NO_BROADCAST,
    },
    manifest::{
        EXECUTION_MANIFEST_EXPIRED, EXECUTION_MANIFEST_INVALIDATED_CHAIN_CHANGE,
        EXECUTION_MANIFEST_SUPERSEDED, EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
    },
    monitor::{ChainPeak, ClosingObservation, MonitorAction, MonitorError, ObservedCoin},
    preparation::{
        CHAIN_RECHECK_REQUIRED, INVALIDATED_CHAIN_CHANGE, OFFLINE_VERIFIED_AWAITING_APPROVAL,
    },
    rpc::{CoinSpend, RpcChainView, WatchtowerChainProvider},
};

fn approval(
    store: &WatchtowerStore,
    closing_coin_id: [u8; 32],
    seed: u8,
    approver_id: &str,
    failure_domain: &str,
    issued_at: u64,
    expires_at: u64,
) -> SignedApproval {
    let preparation = store
        .offline_preparation(closing_coin_id)
        .expect("preparation query")
        .expect("offline preparation");
    let secret_key = key(seed);
    let statement = ApprovalStatement::for_preparation(
        &preparation,
        approver_id,
        failure_domain,
        public_key_bytes(&secret_key),
        issued_at,
        expires_at,
        [seed; 32],
    );
    SignedApproval::sign(statement, &secret_key).expect("signed approval")
}

fn prepare_and_approve(
    store: &mut WatchtowerStore,
    observed: &ClosingObservation,
    approval_expiry: u64,
) -> [u8; 32] {
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(observed, &fee, 11)
        .expect("offline preparation");
    for (seed, id, domain) in [(10, "operator-a", "vps-a"), (11, "operator-b", "vps-b")] {
        let signed = approval(store, closing_id, seed, id, domain, 11, approval_expiry);
        store
            .submit_challenge_approval(&signed, 12)
            .expect("approval");
    }
    closing_id
}

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

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

fn entries(count: usize) -> Vec<LedgerEntry> {
    (0..count)
        .map(|index| LedgerEntry {
            merchant_puzzle_hash: [0x20 + index as u8; 32],
            merchant_receipt_public_key: public_key_bytes(&key(3)),
            amount: 100,
            reservation_nonce: [index as u8 + 1; 32],
        })
        .collect()
}

fn package(count: usize, sequence: u64, previous: [u8; 32]) -> RecoveryPackage {
    let terms = terms();
    let entries = entries(count);
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
                &checkpoint.hub_state_hash(&terms).expect("Hub state hash"),
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

fn package_pair() -> (RecoveryPackage, RecoveryPackage) {
    let terms = terms();
    let zero = StateZero::new(&terms)
        .expect("state zero")
        .hash(&terms, &FUNDING_COIN_ID)
        .expect("state zero hash");
    let current = package(1, 1, zero);
    let previous = current
        .official_state
        .checkpoint
        .hash(&terms)
        .expect("current hash");
    (current, package(2, 2, previous))
}

fn funding_puzzle_hash(package: &RecoveryPackage) -> [u8; 32] {
    let mut allocator = Allocator::new();
    let node = node_from_bytes(&mut allocator, &package.funding_puzzle_reveal).expect("reveal");
    tree_hash(&allocator, node).to_bytes()
}

fn observation(
    current: &RecoveryPackage,
    kind: ClosingCoinKind,
    peak_height: u64,
) -> ClosingObservation {
    let birth_height = 1_000;
    let deadline = birth_height + current.channel_terms.challenge_blocks;
    let checkpoint_hash = current
        .official_state
        .checkpoint
        .hash(&current.channel_terms)
        .expect("checkpoint hash");
    let terms_hash = current.channel_terms.hash().expect("terms hash");
    let deadline_commitment = match kind {
        ClosingCoinKind::Initial => [0; 8],
        ClosingCoinKind::Subsequent => deadline.to_be_bytes(),
    };
    let commitment = closing_state_hash(
        &current.channel_terms.network_id,
        &FUNDING_COIN_ID,
        &terms_hash,
        &deadline_commitment,
        &checkpoint_hash,
    );
    let hashes = module_hashes();
    let module_hash = match kind {
        ClosingCoinKind::Initial => hashes.initial_closing,
        ClosingCoinKind::Subsequent => hashes.subsequent_closing,
    };
    let puzzle_hash = one_arg_puzzle_hash(module_hash, &commitment);
    let closing_parent = match kind {
        ClosingCoinKind::Initial => FUNDING_COIN_ID,
        ClosingCoinKind::Subsequent => [0x55; 32],
    };
    let coin = Coin::new(
        closing_parent.into(),
        puzzle_hash.into(),
        current.funding_amount,
    );
    ClosingObservation {
        network_id: current.channel_terms.network_id,
        synced: true,
        peak: ChainPeak {
            height: peak_height,
            header_hash: [0x99; 32],
        },
        funding_coin: ObservedCoin {
            coin_id: FUNDING_COIN_ID,
            parent_coin_id: [0x10; 32],
            puzzle_hash: funding_puzzle_hash(current),
            amount: current.funding_amount,
            birth_height: 900,
            spent_height: Some(birth_height),
        },
        closing_coin: Some(ObservedCoin {
            coin_id: coin.coin_id().to_bytes(),
            parent_coin_id: closing_parent,
            puzzle_hash,
            amount: current.funding_amount,
            birth_height,
            spent_height: None,
        }),
        closing_coin_kind: Some(kind),
        current_state_sequence: Some(1),
        current_checkpoint_hash: Some(checkpoint_hash),
        initial_birth_height: Some(birth_height),
        challenge_deadline_height: Some(deadline),
        terminal_finalized: false,
    }
}

fn open_observation(package: &RecoveryPackage, peak_height: u64) -> ClosingObservation {
    ClosingObservation {
        network_id: package.channel_terms.network_id,
        synced: true,
        peak: ChainPeak {
            height: peak_height,
            header_hash: [0x98; 32],
        },
        funding_coin: ObservedCoin {
            coin_id: FUNDING_COIN_ID,
            parent_coin_id: [0x10; 32],
            puzzle_hash: funding_puzzle_hash(package),
            amount: package.funding_amount,
            birth_height: 900,
            spent_height: None,
        },
        closing_coin: None,
        closing_coin_kind: None,
        current_state_sequence: None,
        current_checkpoint_hash: None,
        initial_birth_height: None,
        challenge_deadline_height: None,
        terminal_finalized: false,
    }
}

fn state_zero_observation(latest: &RecoveryPackage, peak_height: u64) -> ClosingObservation {
    let zero = StateZero::new(&latest.channel_terms).expect("State 0");
    let zero_hash = zero
        .hash(&latest.channel_terms, &FUNDING_COIN_ID)
        .expect("State 0 hash");
    let terms_hash = latest.channel_terms.hash().expect("terms hash");
    let commitment = closing_state_hash(
        &latest.channel_terms.network_id,
        &FUNDING_COIN_ID,
        &terms_hash,
        &[0; 8],
        &zero_hash,
    );
    let puzzle_hash = one_arg_puzzle_hash(module_hashes().initial_closing, &commitment);
    let coin = Coin::new(
        FUNDING_COIN_ID.into(),
        puzzle_hash.into(),
        latest.funding_amount,
    );
    ClosingObservation {
        network_id: latest.channel_terms.network_id,
        synced: true,
        peak: ChainPeak {
            height: peak_height,
            header_hash: [0x99; 32],
        },
        funding_coin: ObservedCoin {
            coin_id: FUNDING_COIN_ID,
            parent_coin_id: [0x10; 32],
            puzzle_hash: funding_puzzle_hash(latest),
            amount: latest.funding_amount,
            birth_height: 900,
            spent_height: Some(1_000),
        },
        closing_coin: Some(ObservedCoin {
            coin_id: coin.coin_id().to_bytes(),
            parent_coin_id: FUNDING_COIN_ID,
            puzzle_hash,
            amount: latest.funding_amount,
            birth_height: 1_000,
            spent_height: None,
        }),
        closing_coin_kind: Some(ClosingCoinKind::Initial),
        current_state_sequence: Some(0),
        current_checkpoint_hash: Some(zero_hash),
        initial_birth_height: Some(1_000),
        challenge_deadline_height: Some(1_050),
        terminal_finalized: false,
    }
}

fn store_with_packages() -> (WatchtowerStore, RecoveryPackage, RecoveryPackage) {
    let (current, latest) = package_pair();
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("current package");
    store
        .accept_package(&latest.canonical_bytes(), 2)
        .expect("latest package");
    (store, current, latest)
}

#[test]
fn stale_initial_and_subsequent_closing_create_idempotent_non_broadcast_plans() {
    for kind in [ClosingCoinKind::Initial, ClosingCoinKind::Subsequent] {
        let (mut store, current, _) = store_with_packages();
        let observed = observation(&current, kind, 1_020);
        let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
        let first = store
            .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
            .expect("first observation");
        assert_eq!(first.action, MonitorAction::ChallengePlanned);
        let challenge = first.challenge.expect("simulation");
        assert_eq!(
            (
                challenge.current_state_sequence,
                challenge.latest_state_sequence
            ),
            (1, 2)
        );
        assert_eq!(challenge.challenge_deadline_height, 1_050);
        assert!(!challenge.spend_bundle_created);
        assert!(!challenge.broadcast_ready);
        assert!(!challenge.chain_broadcast);

        let persisted = store
            .challenge_plan(closing_id)
            .expect("plan query")
            .expect("persisted plan");
        assert_eq!(persisted.status, "SIMULATED_ONLY");
        assert_eq!(persisted.attempt_count, 0);

        let repeated = store
            .observe_chain(FUNDING_COIN_ID, Ok(observed), 11)
            .expect("repeated observation");
        assert_eq!(repeated.action, MonitorAction::ChallengeAlreadyPlanned);
    }
}

#[test]
fn state_zero_initial_closing_is_challenged_by_the_latest_complete_package() {
    let (mut store, _, latest) = store_with_packages();
    let decision = store
        .observe_chain(
            FUNDING_COIN_ID,
            Ok(state_zero_observation(&latest, 1_020)),
            10,
        )
        .expect("State 0 observation");
    assert_eq!(decision.action, MonitorAction::ChallengePlanned);
    let challenge = decision.challenge.expect("State 0 challenge");
    assert_eq!(
        (
            challenge.current_state_sequence,
            challenge.latest_state_sequence
        ),
        (0, 2)
    );
    assert_eq!(challenge.agg_sig_condition_count, 3);
    assert!(!challenge.spend_bundle_created);
    assert!(!challenge.chain_broadcast);
}

#[test]
fn challenge_plan_prepares_an_offline_verified_approval_record() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    let bundle = store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");
    assert!(bundle.report().spend_bundle_created);
    assert!(!bundle.report().broadcast_enabled);
    assert!(!bundle.report().broadcast_ready);
    assert!(!bundle.report().chain_broadcast);

    let prepared = store
        .offline_preparation(closing_id)
        .expect("preparation query")
        .expect("persisted preparation");
    assert_eq!(prepared.status, OFFLINE_VERIFIED_AWAITING_APPROVAL);
    assert_eq!(prepared.snapshot.peak_height, 1_020);
    assert_eq!(prepared.snapshot.peak_header_hash, [0x99; 32]);
    assert_eq!(prepared.fee_mojo, 2);
    assert_eq!(prepared.report["broadcast_enabled"], false);
    assert_eq!(prepared.report["broadcast_ready"], false);
    assert_eq!(prepared.report["chain_broadcast"], false);
    assert!(prepared.invalidation_reason.is_none());

    let repeated = store
        .observe_chain(FUNDING_COIN_ID, Ok(observed), 12)
        .expect("same observation");
    assert_eq!(repeated.action, MonitorAction::ChallengeAlreadyPlanned);
    assert_eq!(
        store
            .offline_preparation(closing_id)
            .expect("query")
            .expect("preparation")
            .status,
        OFFLINE_VERIFIED_AWAITING_APPROVAL
    );
}

#[test]
fn offline_preparation_requires_a_plan_and_is_invalidated_by_a_new_peak() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    assert!(store.prepare_offline_challenge(&observed, &fee, 9).is_err());
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");

    let mut advanced = observed;
    advanced.peak.height += 1;
    advanced.peak.header_hash = [0x9a; 32];
    store
        .observe_chain(FUNDING_COIN_ID, Ok(advanced), 12)
        .expect("advanced peak");
    let prepared = store
        .offline_preparation(closing_id)
        .expect("query")
        .expect("preparation");
    assert_eq!(prepared.status, INVALIDATED_CHAIN_CHANGE);
    assert!(prepared.invalidation_reason.is_some());
}

#[test]
fn rpc_unknown_marks_offline_preparation_for_a_full_chain_recheck() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");
    store
        .observe_chain(
            FUNDING_COIN_ID,
            Err(MonitorError::Unknown("RPC timeout".into())),
            12,
        )
        .expect("unknown decision");
    let prepared = store
        .offline_preparation(closing_id)
        .expect("query")
        .expect("preparation");
    assert_eq!(prepared.status, CHAIN_RECHECK_REQUIRED);
    assert!(
        prepared
            .invalidation_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("fresh snapshot"))
    );
}

#[test]
fn two_distinct_approvers_and_failure_domains_reach_a_non_broadcast_dual_approval() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");

    let first = approval(&store, closing_id, 10, "operator-a", "vps-a", 11, 30);
    let partial = store
        .submit_challenge_approval(&first, 12)
        .expect("first approval");
    assert_eq!(partial.status, PARTIALLY_APPROVED);
    assert_eq!(
        (partial.approver_count, partial.failure_domain_count),
        (1, 1)
    );
    assert_eq!(
        store
            .submit_challenge_approval(&first, 12)
            .expect("idempotent approval"),
        partial
    );

    let second = approval(&store, closing_id, 11, "operator-b", "vps-b", 12, 30);
    let dual = store
        .submit_challenge_approval(&second, 13)
        .expect("second approval");
    assert_eq!(dual.status, DUAL_APPROVED_RECHECK_REQUIRED);
    assert_eq!((dual.approver_count, dual.failure_domain_count), (2, 2));
    assert!(!dual.broadcast_enabled);
    assert!(!dual.broadcast_ready);
    assert!(!dual.chain_broadcast);
    assert_eq!(
        store
            .submit_challenge_approval(&second, 13)
            .expect("idempotent second approval"),
        dual
    );
    assert_eq!(
        store
            .offline_preparation(closing_id)
            .expect("query")
            .expect("preparation")
            .status,
        DUAL_APPROVED_RECHECK_REQUIRED
    );
}

#[test]
fn approval_signature_binding_duplicate_identity_and_failure_domain_are_enforced() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");

    let mut tampered = approval(&store, closing_id, 10, "operator-a", "vps-a", 11, 30);
    tampered.statement.peak_height += 1;
    assert!(store.submit_challenge_approval(&tampered, 12).is_err());

    let first = approval(&store, closing_id, 10, "operator-a", "vps-a", 11, 30);
    store
        .submit_challenge_approval(&first, 12)
        .expect("first approval");
    let duplicate_identity = approval(&store, closing_id, 11, "operator-a", "vps-b", 12, 30);
    assert!(
        store
            .submit_challenge_approval(&duplicate_identity, 13)
            .is_err()
    );
    let duplicate_domain = approval(&store, closing_id, 11, "operator-b", "vps-a", 12, 30);
    assert!(
        store
            .submit_challenge_approval(&duplicate_domain, 13)
            .is_err()
    );
}

#[test]
fn expired_approval_cannot_contribute_to_the_dual_threshold() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");

    let expired = approval(&store, closing_id, 10, "operator-a", "vps-a", 10, 12);
    assert!(store.submit_challenge_approval(&expired, 12).is_err());
    let short_lived = approval(&store, closing_id, 10, "operator-a", "vps-a", 11, 13);
    store
        .submit_challenge_approval(&short_lived, 12)
        .expect("short-lived approval");
    let second = approval(&store, closing_id, 11, "operator-b", "vps-b", 13, 30);
    let status = store
        .submit_challenge_approval(&second, 14)
        .expect("second approval after first expired");
    assert_eq!(status.status, PARTIALLY_APPROVED);
    assert_eq!(status.approver_count, 1);
}

#[test]
fn rpc_unknown_reorg_and_deadline_revoke_dual_approval_and_require_new_preparation() {
    for invalidation in ["unknown", "reorg", "deadline"] {
        let (mut store, current, _) = store_with_packages();
        let observed = observation(&current, ClosingCoinKind::Initial, 1_049);
        let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
        store
            .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
            .expect("D-1 challenge plan");
        let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
        store
            .prepare_offline_challenge(&observed, &fee, 11)
            .expect("D-1 offline preparation");
        for (seed, id, domain) in [(10, "operator-a", "vps-a"), (11, "operator-b", "vps-b")] {
            let signed = approval(&store, closing_id, seed, id, domain, 11, 30);
            store
                .submit_challenge_approval(&signed, 12)
                .expect("approval");
        }

        match invalidation {
            "unknown" => {
                store
                    .observe_chain(
                        FUNDING_COIN_ID,
                        Err(MonitorError::Unknown("RPC timeout".into())),
                        13,
                    )
                    .expect("unknown observation");
            }
            "reorg" => {
                let mut reorg = observed;
                reorg.peak.header_hash = [0x9a; 32];
                store
                    .observe_chain(FUNDING_COIN_ID, Ok(reorg), 13)
                    .expect("same-height reorg observation");
            }
            "deadline" => {
                let mut at_deadline = observed;
                at_deadline.peak.height = 1_050;
                at_deadline.peak.header_hash = [0x9a; 32];
                store
                    .observe_chain(FUNDING_COIN_ID, Ok(at_deadline), 13)
                    .expect("deadline observation");
            }
            _ => unreachable!(),
        }
        let status = store
            .approval_status(closing_id, 13)
            .expect("revoked status");
        assert_eq!(status.status, APPROVAL_REVOKED_CHAIN_CHANGE);
        assert_eq!(status.approver_count, 0);
        let replacement = approval(&store, closing_id, 12, "operator-c", "vps-c", 13, 30);
        assert!(store.submit_challenge_approval(&replacement, 14).is_err());
    }
}

#[test]
fn rebuilding_a_preparation_creates_a_new_epoch_and_rejects_old_approval_replay() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("first preparation");
    let old = approval(&store, closing_id, 10, "operator-a", "vps-a", 11, 30);
    store
        .submit_challenge_approval(&old, 12)
        .expect("old approval");
    let old_preparation_id = old.statement.preparation_id;

    store
        .observe_chain(
            FUNDING_COIN_ID,
            Err(MonitorError::Unknown("RPC timeout".into())),
            13,
        )
        .expect("unknown observation");
    store
        .prepare_offline_challenge(&observed, &fee, 13)
        .expect("rebuilt preparation");
    assert!(store.submit_challenge_approval(&old, 14).is_err());

    let fresh = approval(&store, closing_id, 10, "operator-a", "vps-a", 13, 30);
    assert_ne!(fresh.statement.preparation_id, old_preparation_id);
    assert_eq!(
        store
            .submit_challenge_approval(&fresh, 14)
            .expect("fresh approval")
            .status,
        PARTIALLY_APPROVED
    );
}

#[test]
fn final_recheck_requires_dual_approval_and_persists_a_short_lived_non_broadcast_record() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed.clone()), 10)
        .expect("challenge plan");
    let fee = test_fee_coin([0xf0; 32], 20, key(9), [0xf1; 32], 2).expect("test fee sponsor");
    store
        .prepare_offline_challenge(&observed, &fee, 11)
        .expect("offline preparation");
    assert!(store.perform_final_chain_recheck(&observed, 12).is_err());

    for (seed, id, domain) in [(10, "operator-a", "vps-a"), (11, "operator-b", "vps-b")] {
        let signed = approval(&store, closing_id, seed, id, domain, 11, 100);
        store
            .submit_challenge_approval(&signed, 12)
            .expect("approval");
    }
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    assert_eq!(recheck.status, FINAL_RECHECK_VERIFIED_NO_BROADCAST);
    assert_eq!(recheck.expires_at, 43);
    assert_eq!(recheck.peak_height, 1_020);
    assert_eq!(recheck.peak_header_hash, [0x99; 32]);
    let preparation = store
        .offline_preparation(closing_id)
        .expect("preparation query")
        .expect("preparation");
    assert_eq!(recheck.bundle_commitment, preparation.bundle_commitment);
    assert_eq!(
        first_bundle_commitment(&store, closing_id),
        preparation.bundle_commitment
    );
    assert!(!recheck.broadcast_enabled);
    assert!(!recheck.broadcast_ready);
    assert!(!recheck.chain_broadcast);
    assert_eq!(
        store
            .final_chain_recheck(recheck.recheck_id, 13)
            .expect("recheck query")
            .expect("persisted recheck"),
        recheck
    );
    assert_eq!(
        store
            .perform_final_chain_recheck(&observed, 13)
            .expect("idempotent final recheck"),
        recheck
    );
}

fn first_bundle_commitment(store: &WatchtowerStore, closing_id: [u8; 32]) -> [u8; 32] {
    let preparation = store
        .offline_preparation(closing_id)
        .expect("preparation query")
        .expect("preparation");
    let signed = ApprovalStatement::for_preparation(
        &preparation,
        "commitment-inspection",
        "test-only",
        public_key_bytes(&key(20)),
        1,
        2,
        [20; 32],
    );
    signed.bundle_commitment
}

#[test]
fn final_recheck_expiry_is_capped_by_the_earliest_approval_and_then_expires() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 20);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    assert_eq!(recheck.expires_at, 20);
    let expired = store
        .final_chain_recheck(recheck.recheck_id, 20)
        .expect("expired query")
        .expect("persisted expired recheck");
    assert_eq!(expired.status, FINAL_RECHECK_EXPIRED);
    assert!(
        expired
            .invalidation_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("expired"))
    );
    assert!(!expired.broadcast_ready);
}

#[test]
fn final_recheck_rejects_approval_index_columns_that_differ_from_signed_statements() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
    }
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute(
            "UPDATE v36_challenge_approvals SET failure_domain='tampered-domain'
             WHERE closing_coin_id=?1 AND approver_id='operator-a'",
            [closing_id.as_slice()],
        )
        .expect("tamper approval index column");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert!(store.perform_final_chain_recheck(&observed, 13).is_err());
}

#[test]
fn changing_the_persisted_bundle_commitment_invalidates_existing_approvals() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
    }
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute(
            "UPDATE v36_offline_challenge_preparations SET bundle_commitment=?2
             WHERE closing_coin_id=?1",
            rusqlite::params![closing_id.as_slice(), [0xee_u8; 32].as_slice()],
        )
        .expect("tamper bundle commitment");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert!(store.perform_final_chain_recheck(&observed, 13).is_err());
}

#[test]
fn final_recheck_rejects_snapshot_changes_and_the_deadline_boundary() {
    for mutation in ["peak", "header", "spent", "deadline"] {
        let (mut store, current, _) = store_with_packages();
        let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
        prepare_and_approve(&mut store, &observed, 100);
        let mut changed = observed;
        match mutation {
            "peak" => changed.peak.height += 1,
            "header" => changed.peak.header_hash = [0x9a; 32],
            "spent" => changed.closing_coin.as_mut().expect("closing").spent_height = Some(1_020),
            "deadline" => changed.peak.height = 1_050,
            _ => unreachable!(),
        }
        assert!(store.perform_final_chain_recheck(&changed, 13).is_err());
    }
}

#[test]
fn rpc_unknown_and_reorg_invalidate_a_persisted_final_recheck() {
    for invalidation in ["unknown", "reorg"] {
        let (mut store, current, _) = store_with_packages();
        let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
        prepare_and_approve(&mut store, &observed, 100);
        let recheck = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck");
        match invalidation {
            "unknown" => {
                store
                    .observe_chain(
                        FUNDING_COIN_ID,
                        Err(MonitorError::Unknown("RPC timeout".into())),
                        14,
                    )
                    .expect("unknown observation");
            }
            "reorg" => {
                let mut reorg = observed;
                reorg.peak.header_hash = [0x9a; 32];
                store
                    .observe_chain(FUNDING_COIN_ID, Ok(reorg), 14)
                    .expect("reorg observation");
            }
            _ => unreachable!(),
        }
        let invalidated = store
            .final_chain_recheck(recheck.recheck_id, 14)
            .expect("invalidated query")
            .expect("persisted invalidated recheck");
        assert_eq!(invalidated.status, FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE);
        assert!(invalidated.invalidation_reason.is_some());
        assert!(!invalidated.broadcast_ready);
    }
}

#[test]
fn current_state_deadline_and_reorg_never_create_a_challenge() {
    let (mut store, current, latest) = store_with_packages();
    let mut current_observation = observation(&latest, ClosingCoinKind::Initial, 1_020);
    current_observation.current_state_sequence = Some(2);
    let current_decision = store
        .observe_chain(FUNDING_COIN_ID, Ok(current_observation), 10)
        .expect("current state");
    assert_eq!(current_decision.action, MonitorAction::ClosingCurrent);
    assert!(current_decision.challenge.is_none());

    let deadline = store
        .observe_chain(
            FUNDING_COIN_ID,
            Ok(observation(&current, ClosingCoinKind::Initial, 1_050)),
            11,
        )
        .expect("deadline");
    assert_eq!(deadline.action, MonitorAction::DeadlinePassed);
    assert!(deadline.challenge.is_none());

    let reorg = store
        .observe_chain(FUNDING_COIN_ID, Ok(open_observation(&latest, 1_049)), 12)
        .expect("reorg");
    assert_eq!(reorg.action, MonitorAction::ReorgPending);
    assert!(reorg.challenge.is_none());
}

#[test]
fn finalized_closing_is_terminal_and_never_creates_a_challenge() {
    let (mut store, current, _) = store_with_packages();
    let mut observed = observation(&current, ClosingCoinKind::Initial, 1_060);
    observed
        .closing_coin
        .as_mut()
        .expect("closing")
        .spent_height = Some(1_050);
    observed.terminal_finalized = true;
    let decision = store
        .observe_chain(FUNDING_COIN_ID, Ok(observed), 10)
        .expect("finalized");
    assert_eq!(decision.action, MonitorAction::Finalized);
    assert!(decision.challenge.is_none());
    assert!(!decision.broadcast_ready);
    assert!(!decision.chain_broadcast);
}

#[test]
fn rpc_unknown_is_persisted_and_does_not_allocate_a_plan() {
    let (mut store, _, _) = store_with_packages();
    let decision = store
        .observe_chain(
            FUNDING_COIN_ID,
            Err(MonitorError::Unknown("RPC timeout".into())),
            10,
        )
        .expect("unknown");
    assert_eq!(decision.action, MonitorAction::Unknown);
    assert!(decision.challenge.is_none());
    assert!(!decision.broadcast_ready);
}

#[test]
fn simulated_broadcast_failure_schedules_a_bounded_retry() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = observed.closing_coin.as_ref().expect("closing").coin_id;
    store
        .observe_chain(FUNDING_COIN_ID, Ok(observed), 10)
        .expect("plan");
    let retry = store
        .record_simulated_broadcast_failure(closing_id, 1_048, 10, "mempool unavailable", 11)
        .expect("retry");
    assert_eq!(retry.status, "RETRY_SCHEDULED");
    assert_eq!(retry.attempt_count, 1);
    assert_eq!(retry.next_retry_height, Some(1_049));
    assert_eq!(retry.last_error.as_deref(), Some("mempool unavailable"));
    assert!(!retry.simulation.spend_bundle_created);
    assert!(!retry.simulation.chain_broadcast);
}

#[test]
fn forged_closing_coin_is_rejected_before_clvm_simulation() {
    let (mut store, current, _) = store_with_packages();
    let mut observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    observed.closing_coin.as_mut().expect("closing").puzzle_hash = [0xff; 32];
    assert!(
        store
            .observe_chain(FUNDING_COIN_ID, Ok(observed), 10)
            .is_err()
    );
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct FundingSolution {
    funding_coin_id: ChiaBytes32,
    state_sequence: Bytes,
    previous_checkpoint_hash: ChiaBytes32,
    manifest_root: ChiaBytes32,
    entry_count: Bytes,
    reserved_total: Bytes,
    user_remainder: Bytes,
    entries: Vec<MockEntry>,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct MockEntry {
    entry_index: Bytes,
    merchant_puzzle_hash: ChiaBytes32,
    merchant_receipt_public_key: Bytes,
    amount: Bytes,
    reservation_nonce: ChiaBytes32,
}

#[derive(Clone)]
struct MockProvider {
    view: RpcChainView,
    funding: ObservedCoin,
    closing: ObservedCoin,
    funding_spend: CoinSpend,
}

impl WatchtowerChainProvider for MockProvider {
    fn chain_view(&self) -> Result<RpcChainView, MonitorError> {
        Ok(self.view.clone())
    }

    fn coin(&self, coin_id: [u8; 32]) -> Result<Option<ObservedCoin>, MonitorError> {
        Ok(if coin_id == self.funding.coin_id {
            Some(self.funding.clone())
        } else if coin_id == self.closing.coin_id {
            Some(self.closing.clone())
        } else {
            None
        })
    }

    fn coin_spend(&self, coin_id: [u8; 32], _: u64) -> Result<CoinSpend, MonitorError> {
        if coin_id == self.funding.coin_id {
            Ok(self.funding_spend.clone())
        } else {
            Err(MonitorError::Unknown("unexpected Coin spend".into()))
        }
    }
}

fn fixed(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}

fn funding_solution_bytes(package: &RecoveryPackage) -> Vec<u8> {
    let checkpoint = &package.official_state.checkpoint;
    let solution = FundingSolution {
        funding_coin_id: package.funding_coin_id.into(),
        state_sequence: fixed(checkpoint.state_sequence),
        previous_checkpoint_hash: checkpoint.previous_checkpoint_hash.into(),
        manifest_root: checkpoint.manifest_root.into(),
        entry_count: fixed(checkpoint.entry_count),
        reserved_total: fixed(checkpoint.reserved_total),
        user_remainder: fixed(checkpoint.user_remainder),
        entries: package
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| MockEntry {
                entry_index: fixed(index as u64),
                merchant_puzzle_hash: entry.merchant_puzzle_hash.into(),
                merchant_receipt_public_key: entry.merchant_receipt_public_key.to_vec().into(),
                amount: fixed(entry.amount),
                reservation_nonce: entry.reservation_nonce.into(),
            })
            .collect(),
    };
    let mut allocator = Allocator::new();
    let node = solution.to_clvm(&mut allocator).expect("solution CLVM");
    node_to_bytes(&allocator, node).expect("solution bytes")
}

fn state_zero_funding_solution_bytes(package: &RecoveryPackage) -> Vec<u8> {
    let zero = StateZero::new(&package.channel_terms).expect("State 0");
    let solution = FundingSolution {
        funding_coin_id: package.funding_coin_id.into(),
        state_sequence: fixed(0),
        previous_checkpoint_hash: [0; 32].into(),
        manifest_root: zero.manifest_root.into(),
        entry_count: fixed(0),
        reserved_total: fixed(0),
        user_remainder: fixed(zero.user_remainder),
        entries: Vec::new(),
    };
    let mut allocator = Allocator::new();
    let node = solution
        .to_clvm(&mut allocator)
        .expect("State 0 solution CLVM");
    node_to_bytes(&allocator, node).expect("State 0 solution bytes")
}

#[test]
fn rpc_poller_derives_the_initial_closing_coin_from_the_confirmed_funding_spend() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let funding = observed.funding_coin.clone();
    let closing = observed.closing_coin.expect("Closing Coin");
    let provider = MockProvider {
        view: RpcChainView {
            network_id: current.channel_terms.network_id,
            synced: true,
            peak: ChainPeak {
                height: 1_020,
                header_hash: [0x99; 32],
            },
        },
        funding,
        closing,
        funding_spend: CoinSpend {
            puzzle_reveal: current.funding_puzzle_reveal.clone(),
            solution: funding_solution_bytes(&current),
        },
    };
    let decision = store
        .poll_chain(&provider, FUNDING_COIN_ID, 10)
        .expect("RPC poll");
    assert_eq!(decision.action, MonitorAction::ChallengePlanned);
    let challenge = decision.challenge.expect("challenge simulation");
    assert_eq!(
        (
            challenge.current_state_sequence,
            challenge.latest_state_sequence
        ),
        (1, 2)
    );
    assert!(!challenge.spend_bundle_created);
    assert!(!challenge.chain_broadcast);
}

#[test]
fn final_recheck_rpc_entrypoint_derives_and_verifies_the_current_chain_snapshot() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let provider = MockProvider {
        view: RpcChainView {
            network_id: current.channel_terms.network_id,
            synced: true,
            peak: observed.peak.clone(),
        },
        funding: observed.funding_coin.clone(),
        closing: observed.closing_coin.clone().expect("Closing Coin"),
        funding_spend: CoinSpend {
            puzzle_reveal: current.funding_puzzle_reveal.clone(),
            solution: funding_solution_bytes(&current),
        },
    };
    let recheck = store
        .poll_final_chain_recheck(&provider, FUNDING_COIN_ID, 13)
        .expect("RPC final recheck");
    assert_eq!(recheck.status, FINAL_RECHECK_VERIFIED_NO_BROADCAST);
    assert_eq!(recheck.peak_header_hash, observed.peak.header_hash);
    assert!(!recheck.broadcast_ready);
    assert!(!recheck.chain_broadcast);
}

#[test]
fn final_recheck_rpc_unknown_revokes_approvals_and_prior_rechecks() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("initial final recheck");
    let provider = MockProvider {
        view: RpcChainView {
            network_id: current.channel_terms.network_id,
            synced: false,
            peak: observed.peak,
        },
        funding: observed.funding_coin,
        closing: observed.closing_coin.expect("Closing Coin"),
        funding_spend: CoinSpend {
            puzzle_reveal: current.funding_puzzle_reveal.clone(),
            solution: funding_solution_bytes(&current),
        },
    };
    assert!(
        store
            .poll_final_chain_recheck(&provider, FUNDING_COIN_ID, 14)
            .is_err()
    );
    assert_eq!(
        store
            .offline_preparation(closing_id)
            .expect("preparation query")
            .expect("preparation")
            .status,
        CHAIN_RECHECK_REQUIRED
    );
    assert_eq!(
        store
            .final_chain_recheck(recheck.recheck_id, 14)
            .expect("recheck query")
            .expect("recheck")
            .status,
        FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE
    );
}

#[test]
fn execution_manifest_requires_active_final_recheck_and_binds_all_execution_hashes() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let closing_id = prepare_and_approve(&mut store, &observed, 100);
    assert!(store.issue_execution_manifest([0x01; 32], 13).is_err());
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("execution manifest");
    assert_eq!(manifest.status, EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST);
    assert_eq!(manifest.recheck_id, recheck.recheck_id);
    assert_eq!(manifest.preparation_id, recheck.preparation_id);
    assert_eq!(manifest.closing_coin_id, closing_id);
    assert_eq!(manifest.bundle_commitment, recheck.bundle_commitment);
    assert_eq!(manifest.approval_set_hash, recheck.approval_set_hash);
    assert_eq!(manifest.expires_at, 24);
    assert!(!manifest.broadcast_enabled);
    assert!(!manifest.broadcast_ready);
    assert!(!manifest.chain_broadcast);
    assert_eq!(
        store
            .issue_execution_manifest(recheck.recheck_id, 14)
            .expect("idempotent manifest"),
        manifest
    );
}

#[test]
fn execution_manifest_expiry_is_capped_by_recheck_and_a_new_manifest_supersedes_old() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 20);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let first = store
        .issue_execution_manifest(recheck.recheck_id, 13)
        .expect("first manifest");
    assert_eq!(first.expires_at, 20);
    let expired = store
        .execution_manifest(first.manifest_id, 20)
        .expect("manifest expiry query")
        .expect("expired manifest");
    assert_eq!(expired.status, EXECUTION_MANIFEST_EXPIRED);

    let second = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("replacement manifest");
    assert_ne!(first.manifest_id, second.manifest_id);
    assert_eq!(second.status, EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST);
    let superseded = store
        .execution_manifest(first.manifest_id, 14)
        .expect("superseded query")
        .expect("first manifest");
    assert_eq!(superseded.status, EXECUTION_MANIFEST_EXPIRED);
    assert!(!second.broadcast_ready);
}

#[test]
fn execution_manifest_is_invalidated_by_rpc_unknown_and_recheck_replacement() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    store
        .observe_chain(
            FUNDING_COIN_ID,
            Err(MonitorError::Unknown("RPC timeout".into())),
            15,
        )
        .expect("unknown observation");
    let invalidated = store
        .execution_manifest(manifest.manifest_id, 15)
        .expect("invalidated query")
        .expect("manifest");
    assert_eq!(
        invalidated.status,
        EXECUTION_MANIFEST_INVALIDATED_CHAIN_CHANGE
    );
    assert!(invalidated.invalidation_reason.is_some());
    assert!(!invalidated.broadcast_ready);

    let (mut replacement_store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut replacement_store, &observed, 100);
    let old_recheck = replacement_store
        .perform_final_chain_recheck(&observed, 13)
        .expect("old recheck");
    let old_manifest = replacement_store
        .issue_execution_manifest(old_recheck.recheck_id, 14)
        .expect("old manifest");
    let new_recheck = replacement_store
        .perform_final_chain_recheck(&observed, 14)
        .expect("new recheck");
    assert_ne!(new_recheck.recheck_id, old_recheck.recheck_id);
    let old_after_recheck = replacement_store
        .execution_manifest(old_manifest.manifest_id, 14)
        .expect("superseded manifest query")
        .expect("old manifest");
    assert_eq!(old_after_recheck.status, EXECUTION_MANIFEST_SUPERSEDED);
}

#[test]
fn execution_manifest_rejects_tampered_final_recheck_bindings() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let recheck_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
        recheck_id = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck")
            .recheck_id;
    }
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute(
            "UPDATE v36_final_chain_rechecks SET bundle_commitment=?2 WHERE recheck_id=?1",
            rusqlite::params![recheck_id.as_slice(), [0xdd_u8; 32].as_slice()],
        )
        .expect("tamper final recheck");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert!(store.issue_execution_manifest(recheck_id, 14).is_err());
}

#[test]
fn execution_authorization_binds_manifest_and_only_records_simulated_submissions() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    let authorization = store
        .issue_execution_authorization(manifest.manifest_id, 15)
        .expect("execution authorization");
    assert_eq!(authorization.status, EXECUTION_AUTHORIZED_SIMULATED_ONLY);
    assert_eq!(authorization.manifest_id, manifest.manifest_id);
    assert_eq!(authorization.recheck_id, manifest.recheck_id);
    assert_eq!(authorization.bundle_commitment, manifest.bundle_commitment);
    assert_eq!(authorization.approval_set_hash, manifest.approval_set_hash);
    assert_eq!(authorization.expires_at, 20);
    assert_eq!(authorization.simulated_submission_count, 0);
    assert!(!authorization.broadcast_enabled);
    assert!(!authorization.broadcast_ready);
    assert!(!authorization.chain_broadcast);
    assert_eq!(
        store
            .issue_execution_authorization(manifest.manifest_id, 15)
            .expect("idempotent authorization"),
        authorization
    );

    let simulated = store
        .simulate_execution_submission(authorization.authorization_id, [0xa1; 32], 16)
        .expect("simulated submission");
    assert_eq!(simulated.submission_nonce, [0xa1; 32]);
    assert_eq!(simulated.consumed_at, 16);
    assert!(!simulated.broadcast_enabled);
    assert!(!simulated.broadcast_ready);
    assert!(!simulated.chain_broadcast);
    let consumed = store
        .execution_authorization(authorization.authorization_id, 16)
        .expect("consumed authorization query")
        .expect("consumed authorization");
    assert_eq!(
        consumed.status,
        EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY
    );
    assert_eq!(consumed.simulated_submission_count, 1);
    assert_eq!(consumed.last_simulated_at, Some(16));
    assert_eq!(
        store
            .simulate_execution_submission(authorization.authorization_id, [0xa1; 32], 17)
            .expect("idempotent simulated submission"),
        simulated
    );
    assert!(
        store
            .simulate_execution_submission(authorization.authorization_id, [0xa2; 32], 17)
            .is_err()
    );
    assert!(
        store
            .issue_execution_authorization(manifest.manifest_id, 17)
            .is_err()
    );
}

#[test]
fn execution_audit_chain_commits_manifest_authorization_and_simulated_receipt() {
    let (mut store, current, _) = store_with_packages();
    assert_eq!(
        store
            .execution_audit_head()
            .expect("empty audit head")
            .event_count,
        0
    );
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    let authorization = store
        .issue_execution_authorization(manifest.manifest_id, 15)
        .expect("authorization");
    let receipt = store
        .simulate_execution_submission(authorization.authorization_id, [0xd1; 32], 16)
        .expect("simulated receipt");
    let verification = store
        .verify_execution_audit_chain()
        .expect("execution audit verification");
    assert!(verification.valid);
    assert_eq!(verification.head.event_count, 3);
    assert_ne!(verification.head.head_hash, [0; 32]);
    assert_eq!(receipt.bundle_commitment, manifest.bundle_commitment);
}

#[test]
fn execution_audit_chain_detects_persisted_event_tampering() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
        let recheck = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck");
        store
            .issue_execution_manifest(recheck.recheck_id, 14)
            .expect("manifest");
        assert!(
            store
                .verify_execution_audit_chain()
                .expect("audit before tampering")
                .valid
        );
    }
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute(
            "UPDATE v36_execution_audit_events SET status='TAMPERED' WHERE event_index=1",
            [],
        )
        .expect("tamper event");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert!(
        !store
            .verify_execution_audit_chain()
            .expect("audit after tampering")
            .valid
    );
}

#[test]
fn execution_audit_write_failures_roll_back_each_execution_state_change() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let recheck_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
        recheck_id = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck")
            .recheck_id;
    }

    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_manifest_audit
             BEFORE INSERT ON v36_execution_audit_events
             WHEN NEW.event_type = 'EXECUTION_MANIFEST_ISSUED'
             BEGIN
               SELECT RAISE(ABORT, 'manifest audit failure');
             END;",
        )
        .expect("manifest failure trigger");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("store with manifest trigger");
    assert!(store.issue_execution_manifest(recheck_id, 14).is_err());
    assert_eq!(
        store
            .execution_audit_head()
            .expect("audit head")
            .event_count,
        0
    );
    drop(store);
    let connection = Connection::open(&path).expect("database inspection");
    let manifest_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM v36_execution_manifests", [], |row| {
            row.get(0)
        })
        .expect("manifest count");
    assert_eq!(manifest_count, 0);
    connection
        .execute_batch("DROP TRIGGER fail_manifest_audit;")
        .expect("remove manifest trigger");
    drop(connection);

    let store = WatchtowerStore::open(&path).expect("store without manifest trigger");
    let manifest = store
        .issue_execution_manifest(recheck_id, 14)
        .expect("manifest after failure removal");
    drop(store);
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_authorization_audit
             BEFORE INSERT ON v36_execution_audit_events
             WHEN NEW.event_type = 'EXECUTION_AUTHORIZATION_ISSUED'
             BEGIN
               SELECT RAISE(ABORT, 'authorization audit failure');
             END;",
        )
        .expect("authorization failure trigger");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("store with authorization trigger");
    assert!(
        store
            .issue_execution_authorization(manifest.manifest_id, 15)
            .is_err()
    );
    assert_eq!(
        store
            .execution_audit_head()
            .expect("audit head")
            .event_count,
        1
    );
    drop(store);
    let connection = Connection::open(&path).expect("database inspection");
    let authorization_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM v36_execution_authorizations",
            [],
            |row| row.get(0),
        )
        .expect("authorization count");
    assert_eq!(authorization_count, 0);
    connection
        .execute_batch("DROP TRIGGER fail_authorization_audit;")
        .expect("remove authorization trigger");
    drop(connection);

    let store = WatchtowerStore::open(&path).expect("store without authorization trigger");
    let authorization = store
        .issue_execution_authorization(manifest.manifest_id, 15)
        .expect("authorization after failure removal");
    drop(store);
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_submission_audit
             BEFORE INSERT ON v36_execution_audit_events
             WHEN NEW.event_type = 'SIMULATED_SUBMISSION_RECORDED'
             BEGIN
               SELECT RAISE(ABORT, 'submission audit failure');
             END;",
        )
        .expect("submission failure trigger");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("store with submission trigger");
    assert!(
        store
            .simulate_execution_submission(authorization.authorization_id, [0xd2; 32], 16)
            .is_err()
    );
    let unconsumed = store
        .execution_authorization(authorization.authorization_id, 16)
        .expect("authorization query")
        .expect("authorization");
    assert_eq!(unconsumed.status, EXECUTION_AUTHORIZED_SIMULATED_ONLY);
    assert_eq!(unconsumed.simulated_submission_count, 0);
    assert!(
        store
            .simulated_submission_receipt(authorization.authorization_id)
            .expect("receipt query")
            .is_none()
    );
    assert_eq!(
        store
            .execution_audit_head()
            .expect("audit head")
            .event_count,
        2
    );
}

#[test]
fn execution_audit_anchor_detects_backup_restore_and_accepts_descendants() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let backup_path: PathBuf = directory.path().join("watchtower-before-anchor.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let manifest_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
        let recheck = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck");
        manifest_id = store
            .issue_execution_manifest(recheck.recheck_id, 14)
            .expect("manifest")
            .manifest_id;
    }
    fs::copy(&path, &backup_path).expect("database backup");

    let store = WatchtowerStore::open(&path).expect("reopen store");
    let authorization = store
        .issue_execution_authorization(manifest_id, 15)
        .expect("authorization");
    let anchor = store.create_execution_audit_anchor(15).expect("anchor");
    let descendant = store
        .simulate_execution_submission(authorization.authorization_id, [0xe1; 32], 16)
        .expect("simulated receipt");
    assert_eq!(descendant.consumed_at, 16);
    let descendant_check = store
        .verify_execution_audit_anchor(&anchor)
        .expect("descendant anchor check");
    assert!(descendant_check.valid);
    assert!(!descendant_check.rollback_detected);
    drop(store);

    let restored = WatchtowerStore::open(&backup_path).expect("restored backup");
    let rollback = restored
        .verify_execution_audit_anchor(&anchor)
        .expect("restored anchor check");
    assert!(!rollback.valid);
    assert!(rollback.rollback_detected);
}

#[test]
fn database_backup_manifest_detects_corruption_and_validates_restored_state() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let backup_path = directory.path().join("watchtower-backup.sqlite3");
    let corrupted_path = directory.path().join("watchtower-corrupted.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("current package");
    store
        .accept_package(&latest.canonical_bytes(), 2)
        .expect("latest package");
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    let anchor = store
        .create_execution_audit_anchor(15)
        .expect("audit anchor");
    let manifest = store
        .create_database_backup(&backup_path, 15, Some(&anchor))
        .expect("backup manifest");
    drop(store);

    let restored =
        WatchtowerStore::verify_database_backup_state(&backup_path, &manifest, Some(&anchor))
            .expect("restored state verification");
    assert!(restored.file_exists);
    assert!(restored.hash_matches);
    assert!(restored.size_matches);
    assert!(restored.audit_valid);
    assert_eq!(restored.anchor_valid, Some(true));

    fs::copy(&backup_path, &corrupted_path).expect("copy backup");
    let mut bytes = fs::read(&corrupted_path).expect("read backup");
    let index = bytes.len() / 2;
    bytes[index] ^= 0x01;
    fs::write(&corrupted_path, bytes).expect("corrupt backup");
    let corrupted = WatchtowerStore::verify_database_backup(&corrupted_path, &manifest)
        .expect("corruption verification");
    assert!(corrupted.file_exists);
    assert!(!corrupted.hash_matches);
    assert!(corrupted.size_matches);
}

#[test]
fn encrypted_backup_rejects_wrong_material_and_supports_key_rotation() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let plaintext = directory.path().join("watchtower-backup.sqlite3");
    let encrypted_old = directory.path().join("watchtower-backup-v1.xhub");
    let encrypted_new = directory.path().join("watchtower-backup-v2.xhub");
    let restored_old = directory.path().join("watchtower-restored-v1.sqlite3");
    let restored_new = directory.path().join("watchtower-restored-v2.sqlite3");
    let corrupted = directory.path().join("watchtower-corrupted.xhub");
    let (current, latest) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("current package");
    store
        .accept_package(&latest.canonical_bytes(), 2)
        .expect("latest package");
    let manifest = store
        .create_database_backup(&plaintext, 3, None)
        .expect("backup manifest");
    let key_id_v1 = [0x11; 32];
    let key_v1 = [0x21; 32];
    let key_id_v2 = [0x12; 32];
    let key_v2 = [0x22; 32];
    let old_hash =
        WatchtowerStore::encrypt_database_backup(&plaintext, &encrypted_old, key_id_v1, &key_v1)
            .expect("encrypt v1");
    assert!(
        WatchtowerStore::decrypt_database_backup(
            &encrypted_old,
            &restored_old,
            key_id_v1,
            &key_v1,
        )
        .is_ok()
    );
    assert_eq!(
        fs::read(&plaintext).expect("plaintext"),
        fs::read(&restored_old).expect("restored")
    );
    assert!(
        WatchtowerStore::decrypt_database_backup(
            &encrypted_old,
            directory.path().join("wrong-key.sqlite3"),
            key_id_v1,
            &key_v2
        )
        .is_err()
    );
    assert!(
        WatchtowerStore::decrypt_database_backup(
            &encrypted_old,
            directory.path().join("wrong-id.sqlite3"),
            key_id_v2,
            &key_v1
        )
        .is_err()
    );

    fs::copy(&encrypted_old, &corrupted).expect("copy encrypted backup");
    let mut bytes = fs::read(&corrupted).expect("read encrypted backup");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&corrupted, bytes).expect("tamper encrypted backup");
    assert!(
        WatchtowerStore::decrypt_database_backup(
            &corrupted,
            directory.path().join("tampered.sqlite3"),
            key_id_v1,
            &key_v1
        )
        .is_err()
    );

    WatchtowerStore::encrypt_database_backup(&restored_old, &encrypted_new, key_id_v2, &key_v2)
        .expect("encrypt v2");
    let new_hash =
        WatchtowerStore::decrypt_database_backup(&encrypted_new, &restored_new, key_id_v2, &key_v2)
            .expect("decrypt v2");
    assert_ne!(old_hash, new_hash);
    assert_eq!(
        fs::read(&plaintext).expect("plaintext"),
        fs::read(&restored_new).expect("rotated restore")
    );
    let restored_manifest =
        WatchtowerStore::verify_database_backup_state(&restored_new, &manifest, None)
            .expect("verify rotated restore");
    assert!(restored_manifest.hash_matches);
    assert!(restored_manifest.audit_valid);
    assert!(backup_replicas_are_consistent(&[
        manifest.clone(),
        restored_manifest.manifest.clone(),
    ]));
    let mut divergent = manifest.clone();
    divergent.audit_head_hash[0] ^= 0x01;
    assert!(!backup_replicas_are_consistent(&[manifest, divergent]));
}

#[test]
fn atomic_encrypted_backup_cleans_temporary_plaintext_and_verifies_before_publish() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-atomic.xhub");
    let restored = directory.path().join("watchtower-atomic-restored.sqlite3");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0x31; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0x41; 32],
    };
    let artifact: EncryptedBackupArtifact = store
        .create_encrypted_database_backup(&encrypted, 2, None, key_id, &keys)
        .expect("atomic encrypted backup");
    assert!(encrypted.exists());
    let temp_files = fs::read_dir(directory.path())
        .expect("backup directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .count();
    assert_eq!(temp_files, 0);
    let verification = WatchtowerStore::restore_encrypted_database_backup(
        &encrypted, &restored, &artifact, None, &keys,
    )
    .expect("atomic restore");
    assert!(verification.audit_valid);
    assert!(restored.exists());
    let restored_temp_files = fs::read_dir(directory.path())
        .expect("restore directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .count();
    assert_eq!(restored_temp_files, 0);
}

#[test]
fn atomic_encrypted_backup_failure_leaves_no_plaintext_or_temp_files() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-failed.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    assert!(store
        .create_encrypted_database_backup(
            &encrypted,
            2,
            None,
            [0x51; 32],
            &RejectingBackupKeys,
        )
        .is_err());
    assert!(!encrypted.exists());
    let leftovers = fs::read_dir(directory.path())
        .expect("backup directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn encrypted_backup_artifact_manifest_is_stable_and_fail_closed() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-manifest.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0x61; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0x71; 32],
    };
    let artifact = store
        .create_encrypted_database_backup(&encrypted, 2, None, key_id, &keys)
        .expect("backup");
    let encoded = encode_encrypted_backup_artifact(&artifact);
    assert_eq!(encoded, encode_encrypted_backup_artifact(&artifact));
    let decoded = decode_encrypted_backup_artifact(&encoded).expect("decode");
    assert_eq!(decoded, artifact);

    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decode_encrypted_backup_artifact(&trailing).is_err());

    let mut tampered = encoded.clone();
    let backup_id_offset = 8 + 4 + ENCRYPTED_BACKUP_DOMAIN.len() + 2;
    tampered[backup_id_offset] ^= 1;
    assert!(decode_encrypted_backup_artifact(&tampered).is_err());
}

#[test]
fn backup_artifact_handoff_is_atomic_idempotent_and_restartable() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-handoff.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0x81; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0x91; 32],
    };
    let artifact = store
        .create_encrypted_database_backup(&encrypted, 2, None, key_id, &keys)
        .expect("backup");
    let bytes = encode_encrypted_backup_artifact(&artifact);
    let received = store
        .record_backup_artifact_handoff(&bytes, 3)
        .expect("receive handoff");
    assert_eq!(received.status, "RECEIVED");
    assert_eq!(
        store
            .record_backup_artifact_handoff(&bytes, 4)
            .expect("idempotent receive"),
        received
    );
    let verified = store
        .verify_backup_artifact_handoff(&bytes, &encrypted, None, &keys, 5)
        .expect("verify handoff");
    assert_eq!(verified.status, BACKUP_HANDOFF_VERIFIED);
    assert_eq!(verified.verified_at, Some(5));
    assert!(store.verify_execution_audit_chain().expect("audit").valid);
    drop(store);
    let restarted = WatchtowerStore::open(&path).expect("restart");
    assert_eq!(
        restarted
            .backup_artifact_handoff(received.artifact_hash)
            .expect("handoff query")
            .expect("persisted handoff")
            .status,
        BACKUP_HANDOFF_VERIFIED
    );
}

#[test]
fn backup_artifact_handoff_rejects_changed_envelope_and_cannot_recover() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-reject.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0xa1; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0xb1; 32],
    };
    let artifact = store
        .create_encrypted_database_backup(&encrypted, 2, None, key_id, &keys)
        .expect("backup");
    let bytes = encode_encrypted_backup_artifact(&artifact);
    let received = store
        .record_backup_artifact_handoff(&bytes, 3)
        .expect("receive handoff");
    let mut changed = fs::read(&encrypted).expect("envelope");
    let last = changed.len() - 1;
    changed[last] ^= 1;
    fs::write(&encrypted, changed).expect("tamper envelope");
    assert!(
        store
            .verify_backup_artifact_handoff(&bytes, &encrypted, None, &keys, 4)
            .is_err()
    );
    assert_eq!(
        store
            .backup_artifact_handoff(received.artifact_hash)
            .expect("handoff query")
            .expect("handoff")
            .status,
        BACKUP_HANDOFF_REJECTED
    );
}

#[test]
fn encrypted_backup_replica_comparison_ignores_nonce_and_key_rotation() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let plaintext = directory.path().join("watchtower-backup.sqlite3");
    let encrypted_a = directory.path().join("watchtower-a.xhub");
    let encrypted_b = directory.path().join("watchtower-b.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let manifest = store
        .create_database_backup(&plaintext, 2, None)
        .expect("backup manifest");
    let key_a = [0xc1; 32];
    let key_b = [0xd1; 32];
    let artifact_a = EncryptedBackupArtifact {
        manifest: manifest.clone(),
        envelope_hash: WatchtowerStore::encrypt_database_backup(
            &plaintext,
            &encrypted_a,
            [0xe1; 32],
            &key_a,
        )
        .expect("encrypt replica a"),
        key_id: [0xe1; 32],
    };
    let artifact_b = EncryptedBackupArtifact {
        manifest,
        envelope_hash: WatchtowerStore::encrypt_database_backup(
            &plaintext,
            &encrypted_b,
            [0xf1; 32],
            &key_b,
        )
        .expect("encrypt replica b"),
        key_id: [0xf1; 32],
    };
    assert!(encrypted_backup_replicas_are_consistent(&[
        artifact_a.clone(),
        artifact_b.clone(),
    ]));
    let mut divergent = artifact_b;
    divergent.manifest.audit_head_hash[0] ^= 1;
    assert!(!encrypted_backup_replicas_are_consistent(&[
        artifact_a, divergent
    ]));
}

#[test]
fn verified_handoff_comparison_rejects_unverified_or_divergent_replicas() {
    let base = xhub_watchtower_v3_6::backup::BackupArtifactHandoff {
        artifact_hash: [1; 32],
        backup_id: [2; 32],
        envelope_hash: [3; 32],
        key_id: [4; 32],
        manifest_bytes_hash: [5; 32],
        received_at: 1,
        verified_at: Some(2),
        status: BACKUP_HANDOFF_VERIFIED.to_string(),
        rejection_reason: None,
    };
    assert!(verified_backup_handoffs_are_consistent(&[
        base.clone(),
        base.clone(),
    ]));
    let mut rejected = base.clone();
    rejected.status = BACKUP_HANDOFF_REJECTED.to_string();
    assert!(!verified_backup_handoffs_are_consistent(&[
        base.clone(),
        rejected
    ]));
    let mut divergent = base;
    divergent.manifest_bytes_hash[0] ^= 1;
    assert!(!verified_backup_handoffs_are_consistent(&[
        xhub_watchtower_v3_6::backup::BackupArtifactHandoff {
            manifest_bytes_hash: [5; 32],
            ..divergent.clone()
        },
        divergent,
    ]));
}

#[test]
fn backup_restore_drill_records_audited_result_and_cleans_plaintext() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-drill.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0x12; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0x13; 32],
    };
    let artifact = store
        .create_encrypted_database_backup(&encrypted, 10, None, key_id, &keys)
        .expect("backup");
    let bytes = encode_encrypted_backup_artifact(&artifact);
    store
        .record_backup_artifact_handoff(&bytes, 11)
        .expect("receive");
    store
        .verify_backup_artifact_handoff(&bytes, &encrypted, None, &keys, 12)
        .expect("verify");
    let drill = store
        .run_backup_restore_drill(&bytes, &encrypted, None, &keys, 20, 23)
        .expect("drill");
    assert_eq!(drill.status, BACKUP_RESTORE_DRILL_PASSED);
    assert_eq!(drill.duration_seconds, 3);
    assert!(drill.hash_matches && drill.size_matches && drill.audit_valid);
    assert!(store.verify_execution_audit_chain().expect("audit").valid);
    let leftovers = fs::read_dir(directory.path())
        .expect("directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".drill."))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn backup_retention_only_returns_old_drilled_candidates() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let encrypted = directory.path().join("watchtower-retention.xhub");
    let (current, _) = package_pair();
    let mut store = WatchtowerStore::open(&path).expect("store");
    store
        .accept_package(&current.canonical_bytes(), 1)
        .expect("package");
    let key_id = [0x22; 32];
    let keys = TestBackupKeys {
        key_id,
        key: [0x23; 32],
    };
    let artifact = store
        .create_encrypted_database_backup(&encrypted, 10, None, key_id, &keys)
        .expect("backup");
    let bytes = encode_encrypted_backup_artifact(&artifact);
    store
        .record_backup_artifact_handoff(&bytes, 11)
        .expect("receive");
    store
        .verify_backup_artifact_handoff(&bytes, &encrypted, None, &keys, 12)
        .expect("verify");
    store
        .run_backup_restore_drill(&bytes, &encrypted, None, &keys, 20, 21)
        .expect("drill");
    let mut newest = artifact.manifest.clone();
    newest.created_at = 90;
    newest.backup_id = database_backup_id(
        newest.file_hash,
        newest.size_bytes,
        newest.audit_event_count,
        newest.audit_head_hash,
        newest.anchor_id,
        newest.created_at,
    );
    let candidates = store
        .backup_retention_candidates(
            &[artifact.manifest.clone(), newest],
            BackupRetentionPolicy {
                keep_latest: 1,
                minimum_age_seconds: 50,
            },
            100,
        )
        .expect("retention");
    assert_eq!(candidates, vec![artifact.manifest.backup_id]);
    assert!(encrypted.exists());
}

#[test]
fn execution_authorization_expires_and_new_authorization_supersedes_old() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    let first = store
        .issue_execution_authorization(manifest.manifest_id, 15)
        .expect("first authorization");
    let second = store
        .issue_execution_authorization(manifest.manifest_id, 16)
        .expect("second authorization");
    assert_ne!(first.authorization_id, second.authorization_id);
    assert_eq!(
        store
            .execution_authorization(first.authorization_id, 16)
            .expect("authorization query")
            .expect("first authorization")
            .status,
        EXECUTION_AUTHORIZATION_SUPERSEDED
    );
    assert_eq!(
        store
            .execution_authorization(second.authorization_id, 21)
            .expect("expiry query")
            .expect("second authorization")
            .status,
        EXECUTION_AUTHORIZATION_EXPIRED
    );
    assert!(
        store
            .simulate_execution_submission(second.authorization_id, [0xb1; 32], 21)
            .is_err()
    );
}

#[test]
fn execution_authorization_is_invalidated_by_rpc_unknown() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    prepare_and_approve(&mut store, &observed, 100);
    let recheck = store
        .perform_final_chain_recheck(&observed, 13)
        .expect("final recheck");
    let manifest = store
        .issue_execution_manifest(recheck.recheck_id, 14)
        .expect("manifest");
    let authorization = store
        .issue_execution_authorization(manifest.manifest_id, 15)
        .expect("authorization");
    store
        .observe_chain(
            FUNDING_COIN_ID,
            Err(MonitorError::Unknown("RPC timeout".into())),
            16,
        )
        .expect("unknown observation");
    let invalidated = store
        .execution_authorization(authorization.authorization_id, 16)
        .expect("authorization query")
        .expect("authorization");
    assert_eq!(invalidated.status, EXECUTION_AUTHORIZATION_INVALIDATED);
    assert!(invalidated.invalidation_reason.is_some());
    assert!(
        store
            .simulate_execution_submission(authorization.authorization_id, [0xc1; 32], 16)
            .is_err()
    );
}

#[test]
fn execution_authorization_rejects_tampered_manifest_bindings() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let (current, latest) = package_pair();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let manifest_id;
    {
        let mut store = WatchtowerStore::open(&path).expect("store");
        store
            .accept_package(&current.canonical_bytes(), 1)
            .expect("current package");
        store
            .accept_package(&latest.canonical_bytes(), 2)
            .expect("latest package");
        prepare_and_approve(&mut store, &observed, 100);
        let recheck = store
            .perform_final_chain_recheck(&observed, 13)
            .expect("final recheck");
        manifest_id = store
            .issue_execution_manifest(recheck.recheck_id, 14)
            .expect("manifest")
            .manifest_id;
    }
    let connection = Connection::open(&path).expect("database inspection");
    connection
        .execute(
            "UPDATE v36_execution_manifests SET report_hash=?2 WHERE manifest_id=?1",
            rusqlite::params![manifest_id.as_slice(), [0xdd_u8; 32].as_slice()],
        )
        .expect("tamper manifest");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("reopen store");
    assert!(
        store
            .issue_execution_authorization(manifest_id, 15)
            .is_err()
    );
}

#[test]
fn rpc_poller_treats_malformed_funding_solution_as_unknown() {
    let (mut store, current, _) = store_with_packages();
    let observed = observation(&current, ClosingCoinKind::Initial, 1_020);
    let provider = MockProvider {
        view: RpcChainView {
            network_id: current.channel_terms.network_id,
            synced: true,
            peak: ChainPeak {
                height: 1_020,
                header_hash: [0x99; 32],
            },
        },
        funding: observed.funding_coin,
        closing: observed.closing_coin.expect("Closing Coin"),
        funding_spend: CoinSpend {
            puzzle_reveal: current.funding_puzzle_reveal.clone(),
            solution: vec![0x80],
        },
    };
    let decision = store
        .poll_chain(&provider, FUNDING_COIN_ID, 10)
        .expect("unknown decision");
    assert_eq!(decision.action, MonitorAction::Unknown);
    assert!(decision.challenge.is_none());
}

#[test]
fn rpc_poller_derives_and_challenges_state_zero() {
    let (mut store, _, latest) = store_with_packages();
    let observed = state_zero_observation(&latest, 1_020);
    let provider = MockProvider {
        view: RpcChainView {
            network_id: latest.channel_terms.network_id,
            synced: true,
            peak: ChainPeak {
                height: 1_020,
                header_hash: [0x99; 32],
            },
        },
        funding: observed.funding_coin,
        closing: observed.closing_coin.expect("State 0 Closing Coin"),
        funding_spend: CoinSpend {
            puzzle_reveal: latest.funding_puzzle_reveal.clone(),
            solution: state_zero_funding_solution_bytes(&latest),
        },
    };
    let decision = store
        .poll_chain(&provider, FUNDING_COIN_ID, 10)
        .expect("State 0 RPC poll");
    assert_eq!(decision.action, MonitorAction::ChallengePlanned);
    assert_eq!(
        decision
            .challenge
            .expect("challenge")
            .current_state_sequence,
        0
    );
}

#[test]
fn opening_an_old_monitor_database_migrates_the_state_zero_constraint() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let connection = Connection::open(&path).expect("legacy database");
    connection.execute_batch(
        "PRAGMA foreign_keys = OFF;
         CREATE TABLE v36_challenge_plans (
           closing_coin_id BLOB PRIMARY KEY CHECK(length(closing_coin_id) = 32),
           funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
           current_state_sequence INTEGER NOT NULL CHECK(current_state_sequence > 0),
           latest_state_sequence INTEGER NOT NULL CHECK(latest_state_sequence > current_state_sequence),
           challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > 0),
           simulation_json TEXT NOT NULL,
           status TEXT NOT NULL,
           attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
           next_retry_height INTEGER,
           last_error TEXT,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );",
    ).expect("legacy schema");
    drop(connection);
    let store = WatchtowerStore::open(&path).expect("migrated store");
    drop(store);
    let connection = Connection::open(&path).expect("inspect database");
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='v36_challenge_plans'",
            [],
            |row| row.get(0),
        )
        .expect("schema");
    assert!(schema.contains("current_state_sequence >= 0"));
}

#[test]
fn opening_a_pre_commitment_database_adds_nullable_migration_columns() {
    let directory = tempdir().expect("tempdir");
    let path = directory.path().join("watchtower.sqlite3");
    let connection = Connection::open(&path).expect("legacy database");
    connection
        .execute_batch(
            "CREATE TABLE v36_offline_challenge_preparations (
               closing_coin_id BLOB PRIMARY KEY
             );
             CREATE TABLE v36_final_chain_rechecks (
               recheck_id BLOB PRIMARY KEY
             );",
        )
        .expect("legacy schemas");
    drop(connection);

    let store = WatchtowerStore::open(&path).expect("migrated store");
    drop(store);
    let connection = Connection::open(&path).expect("inspect migrated database");
    for table in [
        "v36_offline_challenge_preparations",
        "v36_final_chain_rechecks",
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("table info");
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("column query")
            .collect::<Result<Vec<_>, _>>()
            .expect("columns");
        assert!(columns.iter().any(|column| column == "bundle_commitment"));
    }
}
