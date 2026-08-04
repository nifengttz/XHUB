use std::{panic::AssertUnwindSafe, panic::catch_unwind};

use chia_protocol::{Bytes32, Coin};
use chia_sdk_test::BlsPair;
use chia_sdk_types::TESTNET11_CONSTANTS;
use clvmr::{Allocator, serde::node_from_bytes};
use proptest::prelude::*;

use super::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

struct Fixture {
    user: BlsPair,
    hub: BlsPair,
    merchant: BlsPair,
    args: ChannelArgs,
    coin: Coin,
    invoice: MerchantInvoice,
    intent: PaymentIntent,
    voucher: PaymentVoucher,
}

fn fixture(seed: u64) -> Fixture {
    let [user, hub, merchant] = BlsPair::range_with_seed::<3>(seed);
    let args = ChannelArgs::new(
        user.pk,
        hub.pk,
        user.puzzle_hash,
        TESTNET11_CONSTANTS.genesis_challenge,
        CLAIM_BEFORE_HEIGHT,
        REFUND_HEIGHT,
    )
    .unwrap();
    let (puzzle_hash, _) = puzzle_reveal(&args).unwrap();
    let coin = Coin::new(Bytes32::from([seed as u8; 32]), puzzle_hash, FUNDING_AMOUNT);
    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            args.genesis_challenge,
            coin.coin_id(),
            Bytes32::from([0x11; 32]),
            merchant.puzzle_hash,
            PAYMENT_EXPIRY_HEIGHT,
            Bytes32::from([0x12; 32]),
        ),
        &hub.sk,
    );
    let solution = ChannelSolution::claim(
        coin.coin_id(),
        invoice.invoice_hash,
        invoice.fields.order_id,
        invoice.fields.merchant_puzzle_hash,
        Bytes32::from([0x13; 32]),
        invoice.fields.payment_expiry_height,
    );
    let commitment = SettlementCommitment::from_channel(&args, &solution).unwrap();
    let intent = PaymentIntent::sign(
        commitment,
        &invoice,
        &args,
        &user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )
    .unwrap();
    let voucher = PaymentVoucher::issue(
        intent.clone(),
        &invoice,
        &args,
        &hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )
    .unwrap();
    Fixture {
        user,
        hub,
        merchant,
        args,
        coin,
        invoice,
        intent,
        voucher,
    }
}

#[test]
fn valid_protocol_objects_round_trip_through_canonical_bytes() {
    let fixture = fixture(701);
    assert_eq!(
        MerchantInvoice::from_bytes(&fixture.invoice.to_bytes()).unwrap(),
        fixture.invoice
    );
    assert_eq!(
        PaymentIntent::from_bytes(&fixture.intent.to_bytes()).unwrap(),
        fixture.intent
    );
    assert_eq!(
        PaymentVoucher::from_bytes(&fixture.voucher.to_bytes()).unwrap(),
        fixture.voucher
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = MerchantInvoice::from_bytes(&bytes);
            let _ = PaymentIntent::from_bytes(&bytes);
            let _ = PaymentVoucher::from_bytes(&bytes);

            let mut allocator = Allocator::new();
            let _ = node_from_bytes(&mut allocator, &bytes);
        }));
        prop_assert!(outcome.is_ok());
    }

    #[test]
    fn mutated_valid_artifacts_remain_well_framed(index in 0usize..310, value in any::<u8>()) {
        let fixture = fixture(702);
        let mut encoded = fixture.invoice.to_bytes();
        encoded[index] = value;
        let decoded = MerchantInvoice::from_bytes(&encoded);
        if let Ok(invoice) = decoded {
            prop_assert_eq!(invoice.to_bytes().len(), MerchantInvoice::ENCODED_LENGTH);
        }
    }

    #[test]
    fn state_machine_rejects_out_of_order_actions(actions in prop::collection::vec(0u8..7, 0..96)) {
        let fixture = fixture(703);
        let channel_id = fixture.voucher.intent.commitment.channel_id;
        let mut store = ChannelStore::open_in_memory().unwrap();
        store.create_channel(channel_id).unwrap();
        store.record_intent(
            channel_id,
            &fixture.intent,
            &fixture.invoice,
            &fixture.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ).unwrap();
        store.issue_voucher(
            channel_id,
            &fixture.invoice,
            &fixture.args,
            &fixture.hub.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ).unwrap();

        for action in actions {
            let before = store.load_channel(channel_id).unwrap().state;
            let expected = expected_transition(before, action);
            let result = match action {
                0 => store.mark_claim_submitted(channel_id),
                1 => store.mark_settled(channel_id),
                2 => store.mark_refundable(channel_id),
                3 => store.mark_refund_submitted(channel_id),
                4 => store.mark_refunded(channel_id),
                5 => store.rollback_claim_after_reorg(channel_id),
                6 => store.rollback_refund_after_reorg(channel_id),
                _ => unreachable!(),
            };
            let after = store.load_channel(channel_id).unwrap().state;
            match expected {
                Some(target) => {
                    prop_assert!(result.is_ok());
                    prop_assert_eq!(after, target);
                }
                None => {
                    prop_assert!(result.is_err());
                    prop_assert_eq!(after, before);
                }
            }
            prop_assert!(!(after == ChannelState::Settled && after == ChannelState::Refunded));
        }
    }
}

fn expected_transition(state: ChannelState, action: u8) -> Option<ChannelState> {
    match (state, action) {
        (ChannelState::VoucherIssued, 0) => Some(ChannelState::ClaimSubmitted),
        (ChannelState::ClaimSubmitted, 1) => Some(ChannelState::Settled),
        (ChannelState::Funded | ChannelState::IntentSigned | ChannelState::VoucherIssued, 2) => {
            Some(ChannelState::Refundable)
        }
        (ChannelState::ClaimSubmitted, 2) => Some(ChannelState::Refundable),
        (ChannelState::Refundable, 3) => Some(ChannelState::RefundSubmitted),
        (ChannelState::RefundSubmitted, 4) => Some(ChannelState::Refunded),
        (ChannelState::Settled, 5) => Some(ChannelState::ClaimSubmitted),
        (ChannelState::Refunded, 6) => Some(ChannelState::RefundSubmitted),
        _ => None,
    }
}

#[test]
fn protocol_height_boundaries_are_rejected_before_signing() {
    let fixture = fixture(704);
    assert!(
        ChannelArgs::new(
            fixture.user.pk,
            fixture.hub.pk,
            fixture.user.puzzle_hash,
            TESTNET11_CONSTANTS.genesis_challenge,
            MAX_PROTOCOL_U64,
            MAX_PROTOCOL_U64.saturating_add(1),
        )
        .is_err()
    );

    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            fixture.args.genesis_challenge,
            fixture.coin.coin_id(),
            Bytes32::from([0x21; 32]),
            fixture.merchant.puzzle_hash,
            u64::MAX,
            Bytes32::from([0x22; 32]),
        ),
        &fixture.hub.sk,
    );
    assert_eq!(
        invoice.verify(&fixture.args, fixture.coin.coin_id(), 0),
        Err(ProtocolError::InvalidField("payment_expiry_height"))
    );
}

#[test]
fn watcher_height_decoding_does_not_truncate_u64_values() {
    let encoded: chia_protocol::Bytes = u64::MAX.to_be_bytes().to_vec().into();
    assert_eq!(crate::service::decode_height(&encoded).unwrap(), u64::MAX);

    let malformed: chia_protocol::Bytes = vec![0; 7].into();
    assert!(matches!(
        crate::service::decode_height(&malformed),
        Err(WatcherError::InvalidHeight)
    ));
}

#[test]
fn duplicate_confirmation_and_reorg_events_are_idempotent() {
    let fixture = fixture(705);
    let channel_id = fixture.voucher.intent.commitment.channel_id;
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store
        .record_intent(
            channel_id,
            &fixture.intent,
            &fixture.invoice,
            &fixture.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    store
        .issue_voucher(
            channel_id,
            &fixture.invoice,
            &fixture.args,
            &fixture.hub.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    store.mark_claim_submitted(channel_id).unwrap();
    store.mark_settled(channel_id).unwrap();

    let duplicate = store.mark_settled(channel_id);
    assert!(matches!(
        duplicate,
        Err(StateStoreError::IllegalStateTransition { .. })
    ));
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::Settled
    );
}

#[test]
fn broadcast_job_recovers_after_each_durable_crash_window() {
    for window in 0..3_u8 {
        let fixture = fixture(710 + u64::from(window));
        let channel_id = fixture.voucher.intent.commitment.channel_id;
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("broadcast.sqlite3");
        let mut store = ChannelStore::open(&path).unwrap();
        store.create_channel(channel_id).unwrap();
        store
            .record_intent(
                channel_id,
                &fixture.intent,
                &fixture.invoice,
                &fixture.args,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
            .unwrap();
        store
            .issue_voucher(
                channel_id,
                &fixture.invoice,
                &fixture.args,
                &fixture.hub.sk,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
            .unwrap();
        let bundle = build_claim_bundle(fixture.coin, &fixture.args, &fixture.voucher).unwrap();
        let key = format!("claim-crash-window-{window}");
        store
            .prepare_broadcast(&BroadcastRequest {
                idempotency_key: &key,
                channel_id,
                kind: BroadcastKind::Claim,
                bundle: &bundle,
                funding_coin_id: fixture.coin.coin_id(),
                fee: None,
                fee_coin_id: None,
            })
            .unwrap();

        if window >= 1 {
            store
                .record_broadcast_attempt(&key, BroadcastState::Pending, None)
                .unwrap();
        }
        if window >= 2 {
            store
                .update_broadcast_state(&key, BroadcastState::Submitted, None)
                .unwrap();
        }
        drop(store);

        let recovered = ChannelStore::open(&path).unwrap();
        let job = recovered
            .recoverable_broadcasts()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let expected = match window {
            0 => BroadcastState::Prepared,
            1 => BroadcastState::Pending,
            2 => BroadcastState::Submitted,
            _ => unreachable!(),
        };
        assert_eq!(job.state, expected);
        assert_eq!(job.spend_bundle_id, bundle.name());
    }
}

#[test]
fn broadcast_idempotency_key_cannot_change_funding_coin() {
    let fixture = fixture(713);
    let channel_id = fixture.voucher.intent.commitment.channel_id;
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store
        .record_intent(
            channel_id,
            &fixture.intent,
            &fixture.invoice,
            &fixture.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    store
        .issue_voucher(
            channel_id,
            &fixture.invoice,
            &fixture.args,
            &fixture.hub.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    let bundle = build_claim_bundle(fixture.coin, &fixture.args, &fixture.voucher).unwrap();
    store
        .prepare_broadcast(&BroadcastRequest {
            idempotency_key: "claim-funding-binding",
            channel_id,
            kind: BroadcastKind::Claim,
            bundle: &bundle,
            funding_coin_id: fixture.coin.coin_id(),
            fee: None,
            fee_coin_id: None,
        })
        .unwrap();
    assert!(matches!(
        store.prepare_broadcast(&BroadcastRequest {
            idempotency_key: "claim-funding-binding",
            channel_id,
            kind: BroadcastKind::Claim,
            bundle: &bundle,
            funding_coin_id: Bytes32::from([0x99; 32]),
            fee: None,
            fee_coin_id: None,
        }),
        Err(StateStoreError::IdempotencyConflict)
    ));
}

#[test]
fn read_only_database_rejects_writes_without_changing_state() {
    let fixture = fixture(706);
    let channel_id = fixture.voucher.intent.commitment.channel_id;
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("readonly.sqlite3");
    let mut store = ChannelStore::open(&path).unwrap();
    store.create_channel(channel_id).unwrap();
    drop(store);

    let mut read_only = ChannelStore::open_read_only(&path).unwrap();
    assert!(read_only.mark_refundable(channel_id).is_err());
    assert_eq!(
        read_only.load_channel(channel_id).unwrap().state,
        ChannelState::Funded
    );
}
