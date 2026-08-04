use chia_protocol::{Bytes32, Coin, SpendBundle};
use chia_sdk_test::{BlsPair, Simulator};
use chia_sdk_types::TESTNET11_CONSTANTS;

use super::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

struct Fixture {
    user: BlsPair,
    stranger: BlsPair,
    args: ChannelArgs,
    coin: Coin,
    solution: ChannelSolution,
    invoice: MerchantInvoice,
    commitment: SettlementCommitment,
    intent: PaymentIntent,
    voucher: PaymentVoucher,
}

fn fixture(sim: &mut Simulator) -> Fixture {
    let [user, hub, merchant, stranger] = BlsPair::range_with_seed::<4>(300);
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
    let invoice_fields = InvoiceFields::new(
        args.genesis_challenge,
        coin.coin_id(),
        Bytes32::from([0x41; 32]),
        merchant.puzzle_hash,
        PAYMENT_EXPIRY_HEIGHT,
        Bytes32::from([0x42; 32]),
    );
    let invoice = MerchantInvoice::issue(invoice_fields, &hub.sk);
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
        commitment.clone(),
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
        stranger,
        args,
        coin,
        solution,
        invoice,
        commitment,
        intent,
        voucher,
    }
}

#[test]
fn voucher_signatures_match_clvm_and_settle_in_simulator() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);

    assert_eq!(
        fixture.commitment.hash(),
        settlement_hash(&fixture.args, &fixture.solution)
    );
    assert_eq!(
        fixture.invoice.merchant_status(0),
        MerchantPaymentStatus::Pending
    );
    assert_eq!(
        fixture.intent.merchant_status(0),
        MerchantPaymentStatus::PendingHub
    );
    assert_eq!(
        fixture
            .voucher
            .merchant_status(
                &fixture.invoice,
                &fixture.args,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
            .unwrap(),
        MerchantPaymentStatus::PaidOffchain
    );

    let funding_coin_id = fixture.coin.coin_id();
    let spend = coin_spend(fixture.coin, &fixture.args, &fixture.solution).unwrap();
    sim.new_transaction(SpendBundle::new(
        vec![spend],
        fixture.voucher.aggregated_signature(),
    ))
    .unwrap();

    let children = sim.children(funding_coin_id);
    assert!(children.iter().any(|state| {
        state.coin.puzzle_hash == fixture.invoice.fields.merchant_puzzle_hash
            && state.coin.amount == MERCHANT_AMOUNT
    }));
    assert!(children.iter().any(|state| {
        state.coin.puzzle_hash == fixture.user.puzzle_hash && state.coin.amount == USER_REMAINDER
    }));
}

type SettlementMutation = Box<dyn Fn(&mut SettlementCommitment)>;

#[test]
fn every_settlement_field_is_signature_bound() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);
    let mutations: Vec<SettlementMutation> = vec![
        Box::new(|c| c.protocol_version += 1),
        Box::new(|c| c.genesis_challenge = Bytes32::from([0x51; 32])),
        Box::new(|c| c.funding_coin_id = Bytes32::from([0x52; 32])),
        Box::new(|c| c.channel_id = Bytes32::from([0x53; 32])),
        Box::new(|c| c.state_number += 1),
        Box::new(|c| c.invoice_hash = Bytes32::from([0x54; 32])),
        Box::new(|c| c.order_id = Bytes32::from([0x55; 32])),
        Box::new(|c| c.merchant_puzzle_hash = Bytes32::from([0x56; 32])),
        Box::new(|c| c.merchant_amount += 1),
        Box::new(|c| c.user_puzzle_hash = Bytes32::from([0x57; 32])),
        Box::new(|c| c.user_remaining_amount += 1),
        Box::new(|c| c.nonce = Bytes32::from([0x58; 32])),
        Box::new(|c| c.payment_expiry_height += 1),
        Box::new(|c| c.claim_before_height += 1),
        Box::new(|c| c.refund_height += 1),
        Box::new(|c| c.fee_policy += 1),
    ];

    for mutate in mutations {
        let mut voucher = fixture.voucher.clone();
        mutate(&mut voucher.intent.commitment);
        assert!(
            voucher
                .verify(
                    &fixture.invoice,
                    &fixture.args,
                    TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                    0,
                )
                .is_err()
        );
    }
}

type InvoiceMutation = Box<dyn Fn(&mut InvoiceFields)>;

#[test]
fn every_invoice_field_is_signature_bound() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);
    let mutations: Vec<InvoiceMutation> = vec![
        Box::new(|f| f.protocol_version += 1),
        Box::new(|f| f.genesis_challenge = Bytes32::from([0x61; 32])),
        Box::new(|f| f.funding_coin_id = Bytes32::from([0x62; 32])),
        Box::new(|f| f.channel_id = Bytes32::from([0x63; 32])),
        Box::new(|f| f.order_id = Bytes32::from([0x64; 32])),
        Box::new(|f| f.merchant_puzzle_hash = Bytes32::from([0x65; 32])),
        Box::new(|f| f.merchant_amount += 1),
        Box::new(|f| f.payment_expiry_height += 1),
        Box::new(|f| f.invoice_nonce = Bytes32::from([0x66; 32])),
    ];

    for mutate in mutations {
        let mut invoice = fixture.invoice.clone();
        mutate(&mut invoice.fields);
        assert!(
            invoice
                .verify(&fixture.args, fixture.coin.coin_id(), 0)
                .is_err()
        );
    }
}

#[test]
fn wrong_context_and_expired_invoice_are_rejected() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);

    let mut wrong_network = fixture.args.clone();
    wrong_network.genesis_challenge = Bytes32::from([0x71; 32]);
    assert_eq!(
        fixture
            .invoice
            .verify(&wrong_network, fixture.coin.coin_id(), 0),
        Err(ProtocolError::WrongNetwork)
    );
    assert_eq!(
        fixture
            .invoice
            .verify(&fixture.args, Bytes32::from([0x72; 32]), 0),
        Err(ProtocolError::WrongFundingCoin)
    );

    let mut wrong_user = fixture.args.clone();
    wrong_user.user_public_key = fixture.stranger.pk;
    assert!(
        fixture
            .intent
            .verify(
                &fixture.invoice,
                &wrong_user,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
            .is_err()
    );

    let mut wrong_hub = fixture.args.clone();
    wrong_hub.hub_public_key = fixture.stranger.pk;
    assert!(
        fixture
            .voucher
            .verify(
                &fixture.invoice,
                &wrong_hub,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                0,
            )
            .is_err()
    );

    assert_eq!(
        fixture.invoice.verify(
            &fixture.args,
            fixture.coin.coin_id(),
            PAYMENT_EXPIRY_HEIGHT + 1,
        ),
        Err(ProtocolError::PaymentExpired)
    );
}

#[test]
fn incomplete_payment_expires_but_issued_voucher_remains_claimable() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);

    assert_eq!(
        fixture.intent.merchant_status(PAYMENT_EXPIRY_HEIGHT),
        MerchantPaymentStatus::PendingHub
    );
    assert_eq!(
        fixture.intent.merchant_status(PAYMENT_EXPIRY_HEIGHT + 1),
        MerchantPaymentStatus::Expired
    );
    assert_eq!(
        fixture
            .voucher
            .merchant_status(
                &fixture.invoice,
                &fixture.args,
                TESTNET11_CONSTANTS.agg_sig_me_additional_data,
                PAYMENT_EXPIRY_HEIGHT + 1,
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
                REFUND_HEIGHT,
            )
            .unwrap(),
        MerchantPaymentStatus::ClaimExpired
    );
}

#[test]
fn wrong_signing_keys_are_rejected() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);

    assert_eq!(
        PaymentIntent::sign(
            fixture.commitment.clone(),
            &fixture.invoice,
            &fixture.args,
            &fixture.stranger.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(ProtocolError::WrongPublicKey("user"))
    );
    assert_eq!(
        PaymentVoucher::issue(
            fixture.intent.clone(),
            &fixture.invoice,
            &fixture.args,
            &fixture.stranger.sk,
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            0,
        ),
        Err(ProtocolError::WrongPublicKey("hub"))
    );
}

#[test]
fn signature_domains_are_distinct() {
    let mut sim = Simulator::new();
    let fixture = fixture(&mut sim);

    assert_ne!(fixture.invoice.invoice_hash, fixture.commitment.hash());
    assert_ne!(
        fixture.commitment.hash(),
        refund_hash(&fixture.args, fixture.coin.coin_id())
    );
    assert_ne!(
        fixture
            .commitment
            .claim_signature_message(TESTNET11_CONSTANTS.agg_sig_me_additional_data),
        fixture.invoice.invoice_hash.to_vec()
    );
}
