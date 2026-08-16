use std::{collections::VecDeque, sync::Mutex};

use chia_bls::SecretKey;
use xhub_hub_v3_6::{
    ChainChannelRegistration, ChainPeak, ChainProviderError, ChainProviderResult, ChainSnapshot,
    ChainStateProvider, ChannelChainState, ChannelRegistration, FundingCoinState, HubStore,
    ReservationRequest,
};
use xhub_protocol_v3_6::{
    ChannelTerms, LedgerEntry, ReservationStatus, public_key_bytes, sign_hash,
};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn registration() -> ChannelRegistration {
    ChannelRegistration {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_birth_height: 100,
        channel_terms: ChannelTerms::new(
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
        .expect("terms"),
    }
}

fn chain_registration() -> ChainChannelRegistration {
    let registration = registration();
    ChainChannelRegistration {
        funding_coin_id: registration.funding_coin_id,
        funding_puzzle_reveal: registration.funding_puzzle_reveal,
        channel_terms: registration.channel_terms,
    }
}

fn request(nonce: u8) -> ReservationRequest {
    let registration = registration();
    let entry = LedgerEntry {
        merchant_puzzle_hash: [nonce.wrapping_add(0x20); 32],
        merchant_receipt_public_key: public_key_bytes(&key(nonce.wrapping_add(10))),
        amount: 100,
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

fn snapshot(height: u64, birth_height: u64) -> ChainSnapshot {
    let registration = registration();
    ChainSnapshot {
        network_id: registration.channel_terms.network_id,
        synced: true,
        peak: Some(ChainPeak {
            height,
            header_hash: [height as u8; 32],
        }),
        funding_coin: FundingCoinState::Confirmed {
            birth_height,
            puzzle_hash: registration.funding_puzzle_hash().expect("puzzle hash"),
            amount: registration.channel_terms.funding_amount,
        },
    }
}

struct ScriptedProvider {
    responses: Mutex<VecDeque<ChainProviderResult<ChainSnapshot>>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<ChainProviderResult<ChainSnapshot>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }

    fn remaining(&self) -> usize {
        self.responses.lock().expect("provider lock").len()
    }
}

impl ChainStateProvider for ScriptedProvider {
    fn snapshot(&self, _funding_coin_id: [u8; 32]) -> ChainProviderResult<ChainSnapshot> {
        self.responses
            .lock()
            .expect("provider lock")
            .pop_front()
            .expect("scripted chain response")
    }
}

fn active_store() -> HubStore {
    let mut store = HubStore::open_in_memory().expect("store");
    store
        .register_channel(&registration(), 1_000)
        .expect("register");
    store
}

#[test]
fn commit_snapshot_enforces_a_minus_one_a_and_a_plus_one() {
    let mut store = active_store();
    let before = ScriptedProvider::new(vec![Ok(snapshot(198, 100)), Ok(snapshot(199, 100))]);
    let accepted = store
        .reserve_with_chain(&request(1), &before, &key(2), 1_001)
        .expect("A-1 accepted");
    assert_eq!(
        accepted.signed_result.result.status,
        ReservationStatus::Signed
    );
    assert_eq!(accepted.signed_result.result.observed_peak_height, 199);
    assert_eq!(before.remaining(), 0);

    let at = ScriptedProvider::new(vec![Ok(snapshot(199, 100)), Ok(snapshot(200, 100))]);
    let rejected = store
        .reserve_with_chain(&request(2), &at, &key(2), 1_002)
        .expect("A rejected");
    assert_eq!(
        rejected.signed_result.result.status,
        ReservationStatus::RejectedFreezing
    );
    assert!(!rejected.signed_result.result.ledger_written);

    let after = ScriptedProvider::new(vec![Ok(snapshot(200, 100)), Ok(snapshot(201, 100))]);
    let rejected = store
        .reserve_with_chain(&request(3), &after, &key(2), 1_003)
        .expect("A+1 rejected");
    assert_eq!(
        rejected.signed_result.result.status,
        ReservationStatus::RejectedFreezing
    );
    let snapshot = store
        .channel_snapshot(FUNDING_COIN_ID)
        .expect("channel snapshot");
    assert_eq!((snapshot.latest_sequence, snapshot.entry_count), (1, 1));
}

#[test]
fn commit_time_race_cannot_use_the_earlier_peak() {
    let mut store = active_store();
    let provider = ScriptedProvider::new(vec![Ok(snapshot(199, 100)), Ok(snapshot(200, 100))]);
    let outcome = store
        .reserve_with_chain(&request(4), &provider, &key(2), 1_001)
        .expect("signed rejection");
    assert_eq!(
        outcome.signed_result.result.status,
        ReservationStatus::RejectedFreezing
    );
    assert_eq!(outcome.signed_result.result.observed_peak_height, 200);
    let channel = store
        .channel_snapshot(FUNDING_COIN_ID)
        .expect("channel snapshot");
    assert_eq!((channel.latest_sequence, channel.entry_count), (0, 0));
}

#[test]
fn node_and_rpc_failures_pause_without_allocating_a_state() {
    let cases = [
        (
            vec![
                Err(ChainProviderError::RpcUnavailable("offline".into())),
                Err(ChainProviderError::RpcUnavailable("offline".into())),
            ],
            ReservationStatus::RpcUnavailable,
            ChannelChainState::RpcUnavailable,
        ),
        (
            vec![
                Ok(ChainSnapshot {
                    synced: false,
                    ..snapshot(150, 100)
                }),
                Ok(ChainSnapshot {
                    synced: false,
                    ..snapshot(151, 100)
                }),
            ],
            ReservationStatus::NodeNotSynced,
            ChannelChainState::NodeNotSynced,
        ),
    ];
    for (index, (responses, expected_status, expected_state)) in cases.into_iter().enumerate() {
        let mut store = active_store();
        let provider = ScriptedProvider::new(responses);
        let outcome = store
            .reserve_with_chain(&request(10 + index as u8), &provider, &key(2), 1_001)
            .expect("signed node status");
        assert_eq!(outcome.signed_result.result.status, expected_status);
        assert!(!outcome.signed_result.result.ledger_written);
        let channel = store
            .channel_snapshot(FUNDING_COIN_ID)
            .expect("channel snapshot");
        assert_eq!(channel.chain_state, expected_state);
        assert_eq!((channel.latest_sequence, channel.entry_count), (0, 0));
    }
}

#[test]
fn wrong_network_missing_peak_or_wrong_coin_is_uncertain() {
    let mut wrong_network = snapshot(150, 100);
    wrong_network.network_id = [0xbb; 32];
    let mut missing_peak = snapshot(150, 100);
    missing_peak.peak = None;
    let mut wrong_coin = snapshot(150, 100);
    wrong_coin.funding_coin = FundingCoinState::Confirmed {
        birth_height: 100,
        puzzle_hash: [0x99; 32],
        amount: 1_000,
    };
    for (index, value) in [wrong_network, missing_peak, wrong_coin]
        .into_iter()
        .enumerate()
    {
        let mut store = active_store();
        let provider = ScriptedProvider::new(vec![Ok(value.clone()), Ok(value)]);
        let outcome = store
            .reserve_with_chain(&request(20 + index as u8), &provider, &key(2), 1_001)
            .expect("signed uncertainty");
        assert_eq!(
            outcome.signed_result.result.status,
            ReservationStatus::ChainStateUncertain
        );
        assert!(!outcome.signed_result.result.ledger_written);
    }
}

#[test]
fn confirmation_depth_activates_only_at_32_blocks() {
    let mut store = HubStore::open_in_memory().expect("store");
    let registration_provider = ScriptedProvider::new(vec![Ok(snapshot(130, 100))]);
    let registered = store
        .register_channel_from_chain(&chain_registration(), &registration_provider, 32, 1_000)
        .expect("register unconfirmed");
    assert_eq!(registered.chain_state, ChannelChainState::Unconfirmed);

    let provider = ScriptedProvider::new(vec![Ok(snapshot(130, 100)), Ok(snapshot(131, 100))]);
    let outcome = store
        .reserve_with_chain(&request(30), &provider, &key(2), 1_001)
        .expect("activation reservation");
    assert_eq!(
        outcome.signed_result.result.status,
        ReservationStatus::Signed
    );
    let channel = store
        .channel_snapshot(FUNDING_COIN_ID)
        .expect("channel snapshot");
    assert_eq!(channel.chain_state, ChannelChainState::Active);
}

#[test]
fn active_missing_coin_enters_reorg_pending() {
    let mut store = active_store();
    let missing = ChainSnapshot {
        funding_coin: FundingCoinState::Missing,
        ..snapshot(151, 100)
    };
    let provider = ScriptedProvider::new(vec![Ok(snapshot(150, 100)), Ok(missing)]);
    let outcome = store
        .reserve_with_chain(&request(40), &provider, &key(2), 1_001)
        .expect("reorg pending");
    assert_eq!(
        outcome.signed_result.result.status,
        ReservationStatus::ChannelReorgPending
    );
    assert_eq!(
        store
            .channel_snapshot(FUNDING_COIN_ID)
            .expect("channel snapshot")
            .chain_state,
        ChannelChainState::ReorgPending
    );
}

#[test]
fn spent_funding_coin_enters_closing_without_new_state() {
    let mut store = active_store();
    let registration = registration();
    let spent = ChainSnapshot {
        funding_coin: FundingCoinState::Spent {
            birth_height: 100,
            spent_height: 150,
            puzzle_hash: registration.funding_puzzle_hash().expect("puzzle hash"),
            amount: registration.channel_terms.funding_amount,
        },
        ..snapshot(151, 100)
    };
    let provider = ScriptedProvider::new(vec![Ok(snapshot(150, 100)), Ok(spent)]);
    let outcome = store
        .reserve_with_chain(&request(45), &provider, &key(2), 1_001)
        .expect("closing result");
    assert_eq!(
        outcome.signed_result.result.status,
        ReservationStatus::ChannelClosing
    );
    assert!(!outcome.signed_result.result.ledger_written);
    let channel = store
        .channel_snapshot(FUNDING_COIN_ID)
        .expect("channel snapshot");
    assert_eq!(channel.chain_state, ChannelChainState::Closing);
    assert_eq!((channel.latest_sequence, channel.entry_count), (0, 0));
}

#[test]
fn effective_a_never_extends_after_active_reorg() {
    let mut store = active_store();
    let reorg = ScriptedProvider::new(vec![Ok(snapshot(159, 100)), Ok(snapshot(160, 150))]);
    let pending = store
        .reserve_with_chain(&request(50), &reorg, &key(2), 1_001)
        .expect("reorg pending");
    assert_eq!(
        pending.signed_result.result.status,
        ReservationStatus::ChannelReorgPending
    );
    let channel = store
        .channel_snapshot(FUNDING_COIN_ID)
        .expect("channel snapshot");
    assert_eq!(channel.funding_birth_height, 150);
    assert_eq!(channel.acceptance_cutoff_height, 200);
    assert_eq!(channel.scheduled_close_height, 260);

    let stable = ScriptedProvider::new(vec![Ok(snapshot(161, 150)), Ok(snapshot(162, 150))]);
    let accepted = store
        .reserve_with_chain(&request(51), &stable, &key(2), 1_002)
        .expect("stable post-reorg reservation");
    assert_eq!(
        accepted.signed_result.result.status,
        ReservationStatus::Signed
    );
    assert_eq!(accepted.signed_result.result.acceptance_cutoff_height, 200);

    let frozen = ScriptedProvider::new(vec![Ok(snapshot(199, 150)), Ok(snapshot(200, 150))]);
    let rejected = store
        .reserve_with_chain(&request(52), &frozen, &key(2), 1_003)
        .expect("effective A rejection");
    assert_eq!(
        rejected.signed_result.result.status,
        ReservationStatus::RejectedFreezing
    );
    assert_eq!(
        store
            .channel_snapshot(FUNDING_COIN_ID)
            .expect("channel snapshot")
            .latest_sequence,
        1
    );
}
