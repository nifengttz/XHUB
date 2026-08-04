use anyhow::Result;
use chia_protocol::{Bytes32, Coin};
use chia_sdk_test::{BlsPair, Simulator};
use chia_sdk_types::TESTNET11_CONSTANTS;
use wall_hub_mvp::*;

const CLAIM_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;

fn advance_to(sim: &mut Simulator, height: u32) {
    while sim.height() < height { sim.create_block(); }
}

fn children(sim: &Simulator, parent: Bytes32) -> Vec<Coin> {
    sim.children(parent).into_iter().map(|record| record.coin).collect()
}

fn main() -> Result<()> {
    println!("WALL-HUB THREE-PARTY INSTANCE TEST");
    println!("backend=local chia simulator");

    let mut sim = Simulator::new();
    let [user, hub, merchant] = BlsPair::range_with_seed::<3>(900);
    println!("[1/6] User, HUB and Merchant test identities created");

    let args = ChannelArgs::new(
        user.pk, hub.pk, user.puzzle_hash,
        TESTNET11_CONSTANTS.genesis_challenge,
        CLAIM_HEIGHT, REFUND_HEIGHT,
    )?;
    let (channel_puzzle_hash, _) = puzzle_reveal(&args)?;
    let funding_coin = sim.new_coin(channel_puzzle_hash, FUNDING_AMOUNT);
    let funding_coin_id = funding_coin.coin_id();
    let channel_id = channel_id(args.genesis_challenge, funding_coin_id);
    println!("[2/6] User funded channel: {funding_coin_id}");

    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            args.genesis_challenge, funding_coin_id,
            Bytes32::from([0x91; 32]), merchant.puzzle_hash, 5,
            Bytes32::from([0x92; 32]),
        ), &hub.sk,
    );
    println!("[3/6] Merchant invoice created; HUB invoice signature verified");

    let solution = ChannelSolution::claim(
        funding_coin_id, invoice.invoice_hash, invoice.fields.order_id,
        merchant.puzzle_hash, Bytes32::from([0x93; 32]), 5,
    );
    let commitment = SettlementCommitment::from_channel(&args, &solution)?;
    let intent = PaymentIntent::sign(
        commitment, &invoice, &args, &user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data, 0,
    )?;
    println!("[4/6] User signed PaymentIntent");

    let voucher = PaymentVoucher::issue(
        intent, &invoice, &args, &hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data, 0,
    )?;
    voucher.verify(&invoice, &args, TESTNET11_CONSTANTS.agg_sig_me_additional_data, 0)?;
    println!("[5/6] HUB co-signed PaymentVoucher; Merchant verified it");

    let mut store = ChannelStore::open_in_memory()?;
    store.create_channel(channel_id)?;
    store.record_intent(channel_id, &voucher.intent, &invoice, &args,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data, 0)?;
    let voucher = store.issue_voucher(channel_id, &invoice, &args, &hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data, 0)?;
    advance_to(&mut sim, CLAIM_HEIGHT as u32);
    let bundle = build_claim_bundle(funding_coin, &args, &voucher)?;
    track_claim_submission(&mut store, channel_id)?;
    sim.new_transaction(bundle)?;
    let outputs = children(&sim, funding_coin_id);
    confirm_claim(&mut store, channel_id, funding_coin_id, &outputs)?;
    println!("[6/6] Merchant submitted Claim and Simulator confirmed it");
    println!("merchant_mojos={}", outputs.iter().find(|c| c.puzzle_hash == merchant.puzzle_hash).map_or(0, |c| c.amount));
    println!("user_change_mojos={}", outputs.iter().find(|c| c.puzzle_hash == user.puzzle_hash).map_or(0, |c| c.amount));
    println!("final_state={:?}", store.load_channel(channel_id)?.state);
    println!("THREE_PARTY_DEMO=PASS");
    Ok(())
}
