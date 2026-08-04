use std::{
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use chia_protocol::{Bytes32, Coin};
use chia_sdk_test::BlsPair;
use chia_sdk_types::TESTNET11_CONSTANTS;
use tempfile::TempDir;

use super::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

struct SigningContext {
    user: BlsPair,
    hub: BlsPair,
    merchant: BlsPair,
    args: ChannelArgs,
    coin: Coin,
}

#[derive(Clone)]
struct Artifacts {
    invoice: MerchantInvoice,
    intent: PaymentIntent,
    voucher: PaymentVoucher,
}

fn signing_context(seed: u64) -> SigningContext {
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
    SigningContext {
        user,
        hub,
        merchant,
        args,
        coin,
    }
}

fn artifacts(context: &SigningContext, order: u8, nonce: u8) -> Artifacts {
    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            context.args.genesis_challenge,
            context.coin.coin_id(),
            Bytes32::from([order; 32]),
            context.merchant.puzzle_hash,
            PAYMENT_EXPIRY_HEIGHT,
            Bytes32::from([order.wrapping_add(1); 32]),
        ),
        &context.hub.sk,
    );
    let solution = ChannelSolution::claim(
        context.coin.coin_id(),
        invoice.invoice_hash,
        invoice.fields.order_id,
        invoice.fields.merchant_puzzle_hash,
        Bytes32::from([nonce; 32]),
        invoice.fields.payment_expiry_height,
    );
    let commitment = SettlementCommitment::from_channel(&context.args, &solution).unwrap();
    let intent = PaymentIntent::sign(
        commitment,
        &invoice,
        &context.args,
        &context.user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )
    .unwrap();
    let voucher = PaymentVoucher::issue(
        intent.clone(),
        &invoice,
        &context.args,
        &context.hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )
    .unwrap();
    Artifacts {
        invoice,
        intent,
        voucher,
    }
}

fn database_path(temp_dir: &TempDir) -> std::path::PathBuf {
    temp_dir.path().join("channels.sqlite3")
}

fn open(path: &Path) -> ChannelStore {
    ChannelStore::open(path).unwrap()
}

#[test]
fn intent_and_voucher_binary_encoding_round_trip() {
    let context = signing_context(400);
    let artifacts = artifacts(&context, 0x11, 0x21);

    assert_eq!(
        PaymentIntent::from_bytes(&artifacts.intent.to_bytes()).unwrap(),
        artifacts.intent
    );
    assert_eq!(
        PaymentVoucher::from_bytes(&artifacts.voucher.to_bytes()).unwrap(),
        artifacts.voucher
    );
    assert_eq!(
        MerchantInvoice::from_bytes(&artifacts.invoice.to_bytes()).unwrap(),
        artifacts.invoice
    );
    let mut truncated = artifacts.voucher.to_bytes();
    truncated.pop();
    assert!(PaymentVoucher::from_bytes(&truncated).is_err());
}

#[test]
fn broadcast_job_is_idempotent_and_survives_restart() {
    let temp_dir = TempDir::new().unwrap();
    let path = database_path(&temp_dir);
    let context = signing_context(405);
    let artifacts = artifacts(&context, 0x19, 0x29);
    let channel_id = artifacts.intent.commitment.channel_id;
    let bundle = build_claim_bundle(context.coin, &context.args, &artifacts.voucher).unwrap();

    let mut store = open(&path);
    store.create_channel(channel_id).unwrap();
    let first = store
        .prepare_broadcast(&BroadcastRequest {
            idempotency_key: "claim:405",
            channel_id,
            kind: BroadcastKind::Claim,
            bundle: &bundle,
            funding_coin_id: context.coin.coin_id(),
            fee: Some(7),
            fee_coin_id: None,
        })
        .unwrap();
    let repeated = store
        .prepare_broadcast(&BroadcastRequest {
            idempotency_key: "claim:405",
            channel_id,
            kind: BroadcastKind::Claim,
            bundle: &bundle,
            funding_coin_id: context.coin.coin_id(),
            fee: Some(7),
            fee_coin_id: None,
        })
        .unwrap();
    assert_eq!(first, repeated);
    store
        .record_broadcast_attempt("claim:405", BroadcastState::Pending, None)
        .unwrap();
    drop(store);

    let restarted = open(&path);
    let recovered = restarted.load_broadcast("claim:405").unwrap().unwrap();
    assert_eq!(recovered.spend_bundle_id, bundle.name());
    assert_eq!(recovered.attempts, 1);
    assert_eq!(restarted.recoverable_broadcasts().unwrap(), vec![recovered]);
    let metrics = restarted.metrics().unwrap();
    assert_eq!(metrics.channels, 1);
    assert_eq!(metrics.broadcast_jobs, 1);
    assert_eq!(metrics.broadcast_attempts, 1);
    assert!(!restarted.list_audit_events().unwrap().is_empty());
}

#[test]
fn duplicate_order_and_nonce_are_rejected() {
    let context = signing_context(410);
    let first = artifacts(&context, 0x12, 0x22);
    let duplicate_order = artifacts(&context, 0x12, 0x23);
    let duplicate_nonce = artifacts(&context, 0x13, 0x22);
    let channel_id = first.intent.commitment.channel_id;
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store
        .record_intent(
            channel_id,
            &first.intent,
            &first.invoice,
            &context.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();

    assert!(matches!(
        store.record_intent(
            channel_id,
            &duplicate_order.intent,
            &duplicate_order.invoice,
            &context.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(StateStoreError::DuplicateOrder)
    ));
    assert!(matches!(
        store.record_intent(
            channel_id,
            &duplicate_nonce.intent,
            &duplicate_nonce.invoice,
            &context.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(StateStoreError::DuplicateNonce)
    ));
}

#[test]
fn voucher_persists_conserved_balances() {
    let context = signing_context(420);
    let artifacts = artifacts(&context, 0x14, 0x24);
    let channel_id = artifacts.intent.commitment.channel_id;
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store
        .record_intent(
            channel_id,
            &artifacts.intent,
            &artifacts.invoice,
            &context.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    let stored_voucher = store
        .issue_voucher(
            channel_id,
            &artifacts.invoice,
            &context.args,
            &context.hub.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    assert_eq!(stored_voucher, artifacts.voucher);

    let record = store.load_channel(channel_id).unwrap();
    assert_eq!(record.state, ChannelState::VoucherIssued);
    assert_eq!(record.merchant_amount, MERCHANT_AMOUNT);
    assert_eq!(record.user_remaining_amount, USER_REMAINDER);
    assert_eq!(
        record.merchant_amount + record.user_remaining_amount,
        FUNDING_AMOUNT
    );
    assert_eq!(record.voucher, Some(artifacts.voucher));
}

#[test]
fn concurrent_vouchers_commit_at_most_once() {
    let temp_dir = TempDir::new().unwrap();
    let path = database_path(&temp_dir);
    let context = signing_context(430);
    let first = artifacts(&context, 0x15, 0x25);
    let second = artifacts(&context, 0x16, 0x26);
    let channel_id = first.intent.commitment.channel_id;
    let mut initial_store = open(&path);
    initial_store.create_channel(channel_id).unwrap();
    drop(initial_store);

    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for artifacts in [first.clone(), second.clone()] {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        let args = context.args.clone();
        let hub_secret_key = context.hub.sk.clone();
        handles.push(thread::spawn(move || {
            let mut store = open(&path);
            barrier.wait();
            store.accept_intent_and_issue_voucher_atomic(
                &artifacts.intent,
                &artifacts.invoice,
                &args,
                &hub_secret_key,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
        }));
    }
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);

    let record = open(&path).load_channel(channel_id).unwrap();
    assert_eq!(record.state, ChannelState::VoucherIssued);
    let stored = record.voucher.unwrap();
    let issued = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(stored, issued);
}

#[test]
fn every_state_and_signed_artifact_survives_restart() {
    let temp_dir = TempDir::new().unwrap();
    let path = database_path(&temp_dir);
    let context = signing_context(440);
    let artifacts = artifacts(&context, 0x17, 0x27);
    let primary_channel_id = artifacts.intent.commitment.channel_id;

    let mut store = open(&path);
    store.create_channel(primary_channel_id).unwrap();
    drop(store);
    assert_eq!(
        open(&path).load_channel(primary_channel_id).unwrap().state,
        ChannelState::Funded
    );

    let mut store = open(&path);
    store
        .record_intent(
            primary_channel_id,
            &artifacts.intent,
            &artifacts.invoice,
            &context.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    drop(store);
    let recovered = open(&path).load_channel(primary_channel_id).unwrap();
    assert_eq!(recovered.state, ChannelState::IntentSigned);
    assert_eq!(recovered.intent, Some(artifacts.intent.clone()));

    let mut store = open(&path);
    let issued_voucher = store
        .issue_voucher(
            primary_channel_id,
            &artifacts.invoice,
            &context.args,
            &context.hub.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        )
        .unwrap();
    assert_eq!(issued_voucher, artifacts.voucher);
    drop(store);
    let recovered = open(&path).load_channel(primary_channel_id).unwrap();
    assert_eq!(recovered.state, ChannelState::VoucherIssued);
    assert_eq!(recovered.voucher, Some(artifacts.voucher.clone()));

    let mut store = open(&path);
    store.mark_claim_submitted(primary_channel_id).unwrap();
    drop(store);
    let recovered = open(&path).load_channel(primary_channel_id).unwrap();
    assert_eq!(recovered.state, ChannelState::ClaimSubmitted);
    assert_eq!(recovered.voucher, Some(artifacts.voucher.clone()));

    let mut store = open(&path);
    store.mark_settled(primary_channel_id).unwrap();
    drop(store);
    let recovered = open(&path).load_channel(primary_channel_id).unwrap();
    assert_eq!(recovered.state, ChannelState::Settled);
    assert_eq!(recovered.voucher, Some(artifacts.voucher));

    let refund_context = signing_context(441);
    let refund_channel_id = channel_id(
        refund_context.args.genesis_challenge,
        refund_context.coin.coin_id(),
    );
    let mut store = open(&path);
    store.create_channel(refund_channel_id).unwrap();
    store.mark_refundable(refund_channel_id).unwrap();
    drop(store);
    assert_eq!(
        open(&path).load_channel(refund_channel_id).unwrap().state,
        ChannelState::Refundable
    );
    let mut store = open(&path);
    store.mark_refund_submitted(refund_channel_id).unwrap();
    drop(store);
    assert_eq!(
        open(&path).load_channel(refund_channel_id).unwrap().state,
        ChannelState::RefundSubmitted
    );
    let mut store = open(&path);
    store.mark_refunded(refund_channel_id).unwrap();
    drop(store);
    assert_eq!(
        open(&path).load_channel(refund_channel_id).unwrap().state,
        ChannelState::Refunded
    );
}

#[test]
fn illegal_transitions_return_explicit_error() {
    let context = signing_context(450);
    let channel_id = channel_id(context.args.genesis_challenge, context.coin.coin_id());
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();

    assert!(matches!(
        store.mark_settled(channel_id),
        Err(StateStoreError::IllegalStateTransition {
            from: ChannelState::Funded,
            to: ChannelState::Settled
        })
    ));
    store.mark_refundable(channel_id).unwrap();
    store.mark_refund_submitted(channel_id).unwrap();
    store.mark_refunded(channel_id).unwrap();
    assert!(matches!(
        store.mark_claim_submitted(channel_id),
        Err(StateStoreError::IllegalStateTransition {
            from: ChannelState::Refunded,
            to: ChannelState::ClaimSubmitted
        })
    ));
}
