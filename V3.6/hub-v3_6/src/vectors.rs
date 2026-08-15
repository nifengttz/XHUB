use chia_bls::SecretKey;
use std::{collections::VecDeque, sync::Mutex};

use serde_json::{Value, json};
use xhub_protocol_v3_6::{
    CanonicalEncode, ChannelTerms, LedgerEntry, StateZero, public_key_bytes, sign_hash,
};

use crate::{
    ChainPeak, ChainProviderResult, ChainSnapshot, ChainStateProvider, ChannelChainState,
    ChannelRegistration, FailurePoint, FundingCoinState, HubError, HubStore, ReservationLookup,
    ReservationRequest, Result,
};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn registration() -> ChannelRegistration {
    let terms = ChannelTerms::new(
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
    .expect("fixed vector channel terms");
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
        merchant_puzzle_hash: [0x20; 32],
        merchant_receipt_public_key: public_key_bytes(&key(10)),
        amount,
        reservation_nonce: [nonce; 32],
    };
    let authorization_hash = entry
        .authorization_hash(&registration.channel_terms, &FUNDING_COIN_ID)
        .expect("fixed vector authorization hash");
    ReservationRequest {
        request_id: [nonce.wrapping_add(0x80); 32],
        funding_coin_id: FUNDING_COIN_ID,
        ledger_entry: entry,
        user_authorization_signature: sign_hash(&key(1), &authorization_hash),
    }
}

fn chain_snapshot(height: u64, birth_height: u64) -> ChainSnapshot {
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
            puzzle_hash: registration
                .funding_puzzle_hash()
                .expect("fixed vector funding puzzle hash"),
            amount: registration.channel_terms.funding_amount,
        },
    }
}

struct VectorProvider {
    responses: Mutex<VecDeque<ChainProviderResult<ChainSnapshot>>>,
}

impl VectorProvider {
    fn new(responses: Vec<ChainProviderResult<ChainSnapshot>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl ChainStateProvider for VectorProvider {
    fn snapshot(&self, _funding_coin_id: [u8; 32]) -> ChainProviderResult<ChainSnapshot> {
        self.responses
            .lock()
            .expect("vector provider lock")
            .pop_front()
            .expect("fixed vector response")
    }
}

pub fn generate_hub_golden_vectors() -> Result<Value> {
    let registration = registration();
    let mut store = HubStore::open_in_memory()?;
    let initial = store.register_channel(&registration, 1_000)?;
    let state_zero_hash = StateZero::new(&registration.channel_terms)?
        .hash(&registration.channel_terms, &registration.funding_coin_id)?;

    let first_request = request(1, 100);
    let first = store.reserve(&first_request, 150, &key(2), 1_001)?;
    let first_retry = store.reserve(&first_request, 151, &key(2), 1_002)?;
    let first_package = first
        .recovery_package
        .as_ref()
        .expect("signed vector has recovery package");

    let conflicting_request = request(1, 101);
    let nonce_conflict = matches!(
        store.reserve(&conflicting_request, 151, &key(2), 1_003),
        Err(HubError::NonceConflict)
    );

    let second = store.reserve(&request(2, 100), 152, &key(2), 1_004)?;
    let second_package = second
        .recovery_package
        .as_ref()
        .expect("signed vector has recovery package");

    let freezing = store.reserve(&request(9, 25), 200, &key(2), 1_005)?;

    let third_request = request(3, 50);
    let crash_injected = matches!(
        store.reserve_with_failure(
            &third_request,
            153,
            &key(2),
            1_006,
            FailurePoint::AfterPreparationCommit,
        ),
        Err(HubError::InjectedFailure(
            FailurePoint::AfterPreparationCommit
        ))
    );
    let pending_before_recovery = matches!(
        store.reservation_status(
            FUNDING_COIN_ID,
            third_request.ledger_entry.reservation_nonce
        )?,
        ReservationLookup::Pending
    );
    let recovered = store.recover_pending(&key(2), 1_007)?;
    let third = recovered
        .first()
        .expect("one fixed pending vector reservation");
    let third_package = third
        .recovery_package
        .as_ref()
        .expect("recovered vector has recovery package");
    let final_snapshot = store.channel_snapshot(FUNDING_COIN_ID)?;

    let mut chain_store = HubStore::open_in_memory()?;
    chain_store.register_channel(&registration, 2_000)?;
    let before_a = VectorProvider::new(vec![
        Ok(chain_snapshot(198, 100)),
        Ok(chain_snapshot(199, 100)),
    ]);
    let chain_accepted =
        chain_store.reserve_with_chain(&request(40, 25), &before_a, &key(2), 2_001)?;
    let at_a = VectorProvider::new(vec![
        Ok(chain_snapshot(199, 100)),
        Ok(chain_snapshot(200, 100)),
    ]);
    let chain_frozen = chain_store.reserve_with_chain(&request(41, 25), &at_a, &key(2), 2_002)?;
    let reorg = VectorProvider::new(vec![
        Ok(chain_snapshot(160, 100)),
        Ok(chain_snapshot(161, 150)),
    ]);
    let chain_reorg = chain_store.reserve_with_chain(&request(42, 25), &reorg, &key(2), 2_003)?;
    let reorg_snapshot = chain_store.channel_snapshot(FUNDING_COIN_ID)?;

    Ok(json!({
        "schema": "xhub-v3-6-hub-vectors-1",
        "protocol_version": "0x0360",
        "decisions": {
            "persistence_key": "(funding_coin_id, latest_sequence, latest_checkpoint_hash)",
            "idempotency_key": "(funding_coin_id, reservation_nonce)",
            "signing_order": "durable PREPARED intent before BLS signing",
            "sqlite": "WAL + synchronous=FULL + BEGIN IMMEDIATE",
            "chain_commit_snapshot": "second provider snapshot while holding BEGIN IMMEDIATE",
            "effective_a": "min(old_A, new_A) after activation"
        },
        "channel": {
            "funding_coin_id": hex::encode(FUNDING_COIN_ID),
            "channel_terms_hash": hex::encode(registration.channel_terms.hash()?),
            "state_zero_hash": hex::encode(state_zero_hash),
            "acceptance_cutoff_height": initial.acceptance_cutoff_height,
            "scheduled_close_height": initial.scheduled_close_height
        },
        "state_1": {
            "checkpoint_hash": hex::encode(first.signed_result.result.checkpoint_hash.expect("hash")),
            "checkpoint_canonical_hex": hex::encode(first_package.official_state.checkpoint.canonical_bytes()),
            "official_state_canonical_hex": hex::encode(first_package.official_state.canonical_bytes()),
            "recovery_package_content_hash": hex::encode(first_package.content_hash()?),
            "reservation_result_canonical_hex": hex::encode(first.signed_result.canonical_bytes()),
            "idempotent_retry_byte_equal": first == first_retry
        },
        "state_2": {
            "checkpoint_hash": hex::encode(second.signed_result.result.checkpoint_hash.expect("hash")),
            "previous_checkpoint_hash": hex::encode(second_package.official_state.checkpoint.previous_checkpoint_hash),
            "entry_count": second_package.entries.len(),
            "recovery_package_content_hash": hex::encode(second_package.content_hash()?),
            "reservation_result_canonical_hex": hex::encode(second.signed_result.canonical_bytes())
        },
        "state_3_after_recovery": {
            "crash_injected": crash_injected,
            "pending_before_recovery": pending_before_recovery,
            "recovered_count": recovered.len(),
            "checkpoint_hash": hex::encode(third.signed_result.result.checkpoint_hash.expect("hash")),
            "entry_count": third_package.entries.len(),
            "recovery_package_content_hash": hex::encode(third_package.content_hash()?)
        },
        "freezing_rejection": {
            "status": freezing.signed_result.result.status as u16,
            "ledger_written": freezing.signed_result.result.ledger_written,
            "reservation_result_canonical_hex": hex::encode(freezing.signed_result.canonical_bytes())
        },
        "nonce_conflict": {
            "detected": nonce_conflict,
            "latest_sequence_unchanged_before_state_2": 1
        },
        "chain_gate": {
            "a_minus_1": {
                "status": chain_accepted.signed_result.result.status as u16,
                "observed_peak_height": chain_accepted.signed_result.result.observed_peak_height,
                "ledger_written": chain_accepted.signed_result.result.ledger_written
            },
            "a": {
                "status": chain_frozen.signed_result.result.status as u16,
                "observed_peak_height": chain_frozen.signed_result.result.observed_peak_height,
                "ledger_written": chain_frozen.signed_result.result.ledger_written
            },
            "active_reorg": {
                "status": chain_reorg.signed_result.result.status as u16,
                "chain_state": match reorg_snapshot.chain_state {
                    ChannelChainState::ReorgPending => "REORG_PENDING",
                    _ => "UNEXPECTED"
                },
                "new_birth_height": reorg_snapshot.funding_birth_height,
                "effective_acceptance_cutoff_height": reorg_snapshot.acceptance_cutoff_height,
                "new_scheduled_close_height": reorg_snapshot.scheduled_close_height
            }
        },
        "final_snapshot": {
            "latest_sequence": final_snapshot.latest_sequence,
            "entry_count": final_snapshot.entry_count,
            "intent_count": store.intent_count(FUNDING_COIN_ID)?
        }
    }))
}
