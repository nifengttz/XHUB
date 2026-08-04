use chia_consensus::validation_error::ErrorCode;
use chia_protocol::{Bytes32, Coin};
use chia_sdk_test::{BlsPair, Simulator, SimulatorError};
use chia_sdk_types::TESTNET11_CONSTANTS;
use tempfile::TempDir;

use super::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

struct Fixture {
    user: BlsPair,
    hub: BlsPair,
    args: ChannelArgs,
    coin: Coin,
    invoice: MerchantInvoice,
    intent: PaymentIntent,
    voucher: PaymentVoucher,
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
            Bytes32::from([seed as u8; 32]),
            merchant.puzzle_hash,
            PAYMENT_EXPIRY_HEIGHT,
            Bytes32::from([seed.wrapping_add(1) as u8; 32]),
        ),
        &hub.sk,
    );
    let solution = ChannelSolution::claim(
        coin.coin_id(),
        invoice.invoice_hash,
        invoice.fields.order_id,
        merchant.puzzle_hash,
        Bytes32::from([seed.wrapping_add(2) as u8; 32]),
        PAYMENT_EXPIRY_HEIGHT,
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
        args,
        coin,
        invoice,
        intent,
        voucher,
    }
}

fn advance_to(sim: &mut Simulator, height: u32) {
    while sim.height() < height {
        sim.create_block();
    }
}

#[test]
fn voucher_replay_is_rejected_across_channel_and_network() {
    let mut sim = Simulator::new();
    let original = fixture(&mut sim, 600);
    let other = fixture(&mut sim, 610);

    assert_eq!(
        original.voucher.verify(
            &other.invoice,
            &original.args,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(ProtocolError::WrongFundingCoin)
    );

    let mut wrong_network = original.args.clone();
    wrong_network.genesis_challenge = Bytes32::from([0x99; 32]);
    assert_eq!(
        original.voucher.verify(
            &original.invoice,
            &wrong_network,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(ProtocolError::WrongNetwork)
    );
    assert!(matches!(
        build_claim_bundle(other.coin, &other.args, &original.voucher),
        Err(SettlementWorkflowError::WrongFundingCoin)
    ));
}

#[test]
fn persisted_voucher_survives_restart_and_claims_at_cutoff() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 620);
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("day6.sqlite3");
    let channel_id = fixture.intent.commitment.channel_id;
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
    drop(store);

    let mut restarted = ChannelStore::open(&path).unwrap();
    let recovered = restarted.load_channel(channel_id).unwrap().voucher.unwrap();
    advance_to(&mut sim, CLAIM_BEFORE_HEIGHT as u32);
    let funding_coin_id = fixture.coin.coin_id();
    let bundle = build_claim_bundle(fixture.coin, &fixture.args, &recovered).unwrap();
    track_claim_submission(&mut restarted, channel_id).unwrap();
    sim.new_transaction(bundle).unwrap();
    let children: Vec<Coin> = sim
        .children(funding_coin_id)
        .into_iter()
        .map(|state| state.coin)
        .collect();
    confirm_claim(&mut restarted, channel_id, funding_coin_id, &children).unwrap();
    assert_eq!(
        restarted.load_channel(channel_id).unwrap().state,
        ChannelState::Settled
    );
}

#[test]
fn refund_height_race_has_one_winner_and_one_output_set() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 630);
    let funding_coin_id = fixture.coin.coin_id();
    let claim = build_claim_bundle(fixture.coin, &fixture.args, &fixture.voucher).unwrap();
    let refund = build_refund_bundle(
        fixture.coin,
        &fixture.args,
        &fixture.user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
    )
    .unwrap();
    advance_to(&mut sim, REFUND_HEIGHT as u32);

    assert!(matches!(
        sim.new_transaction(claim),
        Err(SimulatorError::Validation(
            ErrorCode::AssertBeforeHeightAbsoluteFailed
        ))
    ));
    sim.new_transaction(refund).unwrap();
    let children = sim.children(funding_coin_id);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].coin.puzzle_hash, fixture.user.puzzle_hash);
    assert_eq!(children[0].coin.amount, FUNDING_AMOUNT);
}

#[test]
fn claim_cutoff_race_has_one_winner_and_one_output_set() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 640);
    let funding_coin_id = fixture.coin.coin_id();
    let claim = build_claim_bundle(fixture.coin, &fixture.args, &fixture.voucher).unwrap();
    let refund = build_refund_bundle(
        fixture.coin,
        &fixture.args,
        &fixture.user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
    )
    .unwrap();
    advance_to(&mut sim, CLAIM_BEFORE_HEIGHT as u32);

    assert!(matches!(
        sim.new_transaction(refund),
        Err(SimulatorError::Validation(
            ErrorCode::AssertHeightAbsoluteFailed
        ))
    ));
    sim.new_transaction(claim).unwrap();
    let children = sim.children(funding_coin_id);
    assert_eq!(children.len(), 2);
    assert_eq!(
        children.iter().map(|state| state.coin.amount).sum::<u64>(),
        FUNDING_AMOUNT
    );
}

#[test]
fn voucher_status_becomes_claim_expired_at_refund_height() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim, 650);
    assert_eq!(
        fixture
            .voucher
            .merchant_status(
                &fixture.invoice,
                &fixture.args,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                CLAIM_BEFORE_HEIGHT
            )
            .unwrap(),
        MerchantPaymentStatus::PaidOffchain
    );
    assert_eq!(
        fixture
            .voucher
            .merchant_status(
                &fixture.invoice,
                &fixture.args,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                REFUND_HEIGHT
            )
            .unwrap(),
        MerchantPaymentStatus::ClaimExpired
    );
}
