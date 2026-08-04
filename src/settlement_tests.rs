use chia_consensus::validation_error::ErrorCode;
use chia_protocol::{Bytes32, Coin};
use chia_sdk_coinset::CoinRecord;
use chia_sdk_test::{BlsPair, Simulator, SimulatorError};
use chia_sdk_types::TESTNET11_CONSTANTS;

use super::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;
const FEE_AMOUNT: u64 = 1_000_000;

struct Fixture {
    user: BlsPair,
    hub: BlsPair,
    merchant: BlsPair,
    args: ChannelArgs,
    coin: Coin,
    invoice: MerchantInvoice,
    intent: PaymentIntent,
}

fn fixture(sim: &mut Simulator, seed: u64) -> Fixture {
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
    let coin = sim.new_coin(puzzle_hash, FUNDING_AMOUNT);
    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            args.genesis_challenge,
            coin.coin_id(),
            Bytes32::from([0x41; 32]),
            merchant.puzzle_hash,
            PAYMENT_EXPIRY_HEIGHT,
            Bytes32::from([0x42; 32]),
        ),
        &hub.sk,
    );
    let solution = ChannelSolution::claim(
        coin.coin_id(),
        invoice.invoice_hash,
        invoice.fields.order_id,
        invoice.fields.merchant_puzzle_hash,
        Bytes32::from([0x43; 32]),
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
    Fixture {
        user,
        hub,
        merchant,
        args,
        coin,
        invoice,
        intent,
    }
}

fn persisted_voucher(store: &mut ChannelStore, fixture: &Fixture) -> PaymentVoucher {
    let channel_id = fixture.intent.commitment.channel_id;
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
        .unwrap()
}

fn children(sim: &Simulator, parent: Bytes32) -> Vec<Coin> {
    sim.children(parent)
        .into_iter()
        .map(|state| state.coin)
        .collect()
}

fn advance_to(sim: &mut Simulator, height: u32) {
    while sim.height() < height {
        sim.create_block();
    }
}

#[test]
fn merchant_claims_after_user_and_hub_go_offline() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 500);
    let mut store = ChannelStore::open_in_memory().unwrap();
    let voucher = persisted_voucher(&mut store, &fixture);
    let channel_id = voucher.intent.commitment.channel_id;
    let funding_coin_id = fixture.coin.coin_id();
    let merchant_puzzle_hash = fixture.merchant.puzzle_hash;
    let user_puzzle_hash = fixture.user.puzzle_hash;
    let coin = fixture.coin;
    let args = fixture.args.clone();

    drop(fixture);
    let bundle = build_claim_bundle(coin, &args, &voucher).unwrap();
    track_claim_submission(&mut store, channel_id).unwrap();
    sim.new_transaction(bundle).unwrap();
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::ClaimSubmitted
    );

    let confirmed = children(&sim, funding_coin_id);
    confirm_claim(&mut store, channel_id, funding_coin_id, &confirmed).unwrap();
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::Settled
    );
    assert!(
        confirmed
            .iter()
            .any(|coin| coin.puzzle_hash == merchant_puzzle_hash && coin.amount == MERCHANT_AMOUNT)
    );
    assert!(
        confirmed
            .iter()
            .any(|coin| coin.puzzle_hash == user_puzzle_hash && coin.amount == USER_REMAINDER)
    );
}

#[test]
fn user_refunds_without_voucher_after_refund_height() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 510);
    let channel_id = channel_id(fixture.args.genesis_challenge, fixture.coin.coin_id());
    let funding_coin_id = fixture.coin.coin_id();
    let user_puzzle_hash = fixture.user.puzzle_hash;
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store.mark_refundable(channel_id).unwrap();
    advance_to(&mut sim, REFUND_HEIGHT as u32);

    let bundle = build_refund_bundle(
        fixture.coin,
        &fixture.args,
        &fixture.user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
    )
    .unwrap();
    track_refund_submission(&mut store, channel_id).unwrap();
    sim.new_transaction(bundle).unwrap();
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::RefundSubmitted
    );
    let confirmed = children(&sim, funding_coin_id);
    confirm_refund(
        &mut store,
        channel_id,
        funding_coin_id,
        user_puzzle_hash,
        &confirmed,
    )
    .unwrap();
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::Refunded
    );
    assert_eq!(
        confirmed,
        vec![Coin::new(funding_coin_id, user_puzzle_hash, FUNDING_AMOUNT)]
    );
}

#[test]
fn claim_fails_at_refund_height() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 520);
    let mut store = ChannelStore::open_in_memory().unwrap();
    let voucher = persisted_voucher(&mut store, &fixture);
    let channel_id = voucher.intent.commitment.channel_id;
    let bundle = build_claim_bundle(fixture.coin, &fixture.args, &voucher).unwrap();
    track_claim_submission(&mut store, channel_id).unwrap();
    advance_to(&mut sim, REFUND_HEIGHT as u32);
    assert!(matches!(
        sim.new_transaction(bundle),
        Err(SimulatorError::Validation(
            ErrorCode::AssertBeforeHeightAbsoluteFailed
        ))
    ));
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::ClaimSubmitted
    );
}

#[test]
fn refund_fails_before_refund_height() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 530);
    let channel_id = channel_id(fixture.args.genesis_challenge, fixture.coin.coin_id());
    let mut store = ChannelStore::open_in_memory().unwrap();
    store.create_channel(channel_id).unwrap();
    store.mark_refundable(channel_id).unwrap();
    let bundle = build_refund_bundle(
        fixture.coin,
        &fixture.args,
        &fixture.user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
    )
    .unwrap();
    track_refund_submission(&mut store, channel_id).unwrap();
    advance_to(&mut sim, CLAIM_BEFORE_HEIGHT as u32);
    assert!(matches!(
        sim.new_transaction(bundle),
        Err(SimulatorError::Validation(
            ErrorCode::AssertHeightAbsoluteFailed
        ))
    ));
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::RefundSubmitted
    );
}

#[test]
fn external_fee_coin_preserves_channel_outputs() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 540);
    let mut store = ChannelStore::open_in_memory().unwrap();
    let voucher = persisted_voucher(&mut store, &fixture);
    let funding_coin_id = fixture.coin.coin_id();
    let fee_pair = sim.bls(FEE_AMOUNT);
    assert_ne!(fee_pair.coin.coin_id(), funding_coin_id);
    let claim = build_claim_bundle(fixture.coin, &fixture.args, &voucher).unwrap();
    let fee = build_fee_bundle(fee_pair.coin, &fee_pair.sk).unwrap();
    sim.new_transaction(aggregate_fee_bundle(claim, fee))
        .unwrap();
    let confirmed = children(&sim, funding_coin_id);
    assert_eq!(confirmed.len(), 2);
    assert!(
        confirmed
            .iter()
            .any(|coin| coin.puzzle_hash == fixture.merchant.puzzle_hash
                && coin.amount == MERCHANT_AMOUNT)
    );
    assert!(
        confirmed
            .iter()
            .any(|coin| coin.puzzle_hash == fixture.user.puzzle_hash
                && coin.amount == USER_REMAINDER)
    );
}

#[test]
fn fee_bundle_returns_independent_change() {
    let mut sim = Simulator::new();
    let fee_pair = sim.bls(FEE_AMOUNT);
    let bundle =
        build_fee_bundle_with_change(fee_pair.coin, &fee_pair.sk, 100_000, fee_pair.puzzle_hash)
            .unwrap();
    sim.new_transaction(bundle).unwrap();
    let children = children(&sim, fee_pair.coin.coin_id());
    assert_eq!(
        children,
        vec![Coin::new(
            fee_pair.coin.coin_id(),
            fee_pair.puzzle_hash,
            FEE_AMOUNT - 100_000
        )]
    );
}

#[test]
fn confirmed_settlement_rolls_back_and_persists_reorg_evidence() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 560);
    let mut store = ChannelStore::open_in_memory().unwrap();
    let voucher = persisted_voucher(&mut store, &fixture);
    let channel_id = voucher.intent.commitment.channel_id;
    let funding_coin_id = fixture.coin.coin_id();
    track_claim_submission(&mut store, channel_id).unwrap();
    store.mark_settled(channel_id).unwrap();

    let child = Coin::new(
        funding_coin_id,
        fixture.merchant.puzzle_hash,
        MERCHANT_AMOUNT,
    );
    let previous = ChainObservation {
        tx_id: Bytes32::from([1; 32]),
        funding_coin_id,
        peak_height: 101,
        peak_hash: Bytes32::from([2; 32]),
        funding_coin: None,
        children: vec![CoinRecord {
            coin: child,
            coinbase: false,
            confirmed_block_index: 100,
            spent: false,
            spent_block_index: 0,
            timestamp: 0,
        }],
        confirmed_height: Some(100),
        mempool: MempoolStatus::NotFound,
    };
    let current = ChainObservation {
        tx_id: previous.tx_id,
        funding_coin_id,
        peak_height: 101,
        peak_hash: Bytes32::from([3; 32]),
        funding_coin: None,
        children: Vec::new(),
        confirmed_height: None,
        mempool: MempoolStatus::NotFound,
    };

    assert!(
        reconcile_chain_observation(
            &mut store,
            channel_id,
            Some(&previous),
            &current,
            Some(100_000),
        )
        .unwrap()
    );
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::ClaimSubmitted
    );
    let evidence = store.list_chain_observations(channel_id).unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].reorged);
    assert_eq!(evidence[0].fee, Some(100_000));
}

#[test]
fn confirmation_mismatch_does_not_finalize() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 550);
    let mut store = ChannelStore::open_in_memory().unwrap();
    let voucher = persisted_voucher(&mut store, &fixture);
    let channel_id = voucher.intent.commitment.channel_id;
    track_claim_submission(&mut store, channel_id).unwrap();
    assert!(matches!(
        confirm_claim(&mut store, channel_id, fixture.coin.coin_id(), &[]),
        Err(SettlementWorkflowError::ConfirmationMismatch)
    ));
    assert_eq!(
        store.load_channel(channel_id).unwrap().state,
        ChannelState::ClaimSubmitted
    );
}
