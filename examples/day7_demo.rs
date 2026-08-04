use anyhow::Result;
use chia_protocol::{Bytes32, Coin};
use chia_sdk_test::{BlsPair, Simulator};
use chia_sdk_types::TESTNET11_CONSTANTS;
use wall_hub_mvp::*;

const CLAIM_BEFORE_HEIGHT: u64 = 25;
const REFUND_HEIGHT: u64 = 26;
const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

fn advance_to(sim: &mut Simulator, height: u32) {
    while sim.height() < height {
        sim.create_block();
    }
}

fn confirmed_children(sim: &Simulator, parent: Bytes32) -> Vec<Coin> {
    sim.children(parent)
        .into_iter()
        .map(|state| state.coin)
        .collect()
}

fn run_claim_demo() -> Result<()> {
    println!("SCENARIO A: OFFLINE MERCHANT CLAIM");
    let mut sim = Simulator::new();
    let [user, hub, merchant] = BlsPair::range_with_seed::<3>(700);
    let args = ChannelArgs::new(
        user.pk,
        hub.pk,
        user.puzzle_hash,
        TESTNET11_CONSTANTS.genesis_challenge,
        CLAIM_BEFORE_HEIGHT,
        REFUND_HEIGHT,
    )?;
    let (channel_puzzle_hash, _) = puzzle_reveal(&args)?;
    let funding_coin = sim.new_coin(channel_puzzle_hash, FUNDING_AMOUNT);
    let funding_coin_id = funding_coin.coin_id();
    let merchant_puzzle_hash = merchant.puzzle_hash;
    let user_puzzle_hash = user.puzzle_hash;

    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            args.genesis_challenge,
            funding_coin_id,
            Bytes32::from([0x71; 32]),
            merchant_puzzle_hash,
            PAYMENT_EXPIRY_HEIGHT,
            Bytes32::from([0x72; 32]),
        ),
        &hub.sk,
    );
    let solution = ChannelSolution::claim(
        funding_coin_id,
        invoice.invoice_hash,
        invoice.fields.order_id,
        merchant_puzzle_hash,
        Bytes32::from([0x73; 32]),
        PAYMENT_EXPIRY_HEIGHT,
    );
    let commitment = SettlementCommitment::from_channel(&args, &solution)?;
    let intent = PaymentIntent::sign(
        commitment.clone(),
        &invoice,
        &args,
        &user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )?;
    let voucher = PaymentVoucher::issue(
        intent,
        &invoice,
        &args,
        &hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )?;
    voucher.verify(
        &invoice,
        &args,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )?;

    let channel_id = commitment.channel_id;
    let mut store = ChannelStore::open_in_memory()?;
    store.create_channel(channel_id)?;
    store.record_intent(
        channel_id,
        &voucher.intent,
        &invoice,
        &args,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )?;
    let persisted = store.issue_voucher(
        channel_id,
        &invoice,
        &args,
        &hub.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
        0,
    )?;

    println!("funding_coin_id={funding_coin_id}");
    println!("commitment_hash={}", commitment.hash());
    println!("voucher_signature=VERIFIED");
    drop(user);
    drop(hub);
    println!("user_service=OFFLINE hub_service=OFFLINE");

    advance_to(&mut sim, CLAIM_BEFORE_HEIGHT as u32);
    let submitted_height = sim.height();
    let bundle = build_claim_bundle(funding_coin, &args, &persisted)?;
    let bundle_id = bundle.name();
    track_claim_submission(&mut store, channel_id)?;
    sim.new_transaction(bundle)?;
    let children = confirmed_children(&sim, funding_coin_id);
    confirm_claim(&mut store, channel_id, funding_coin_id, &children)?;

    let merchant_amount = children
        .iter()
        .find(|coin| coin.puzzle_hash == merchant_puzzle_hash)
        .map_or(0, |coin| coin.amount);
    let user_amount = children
        .iter()
        .find(|coin| coin.puzzle_hash == user_puzzle_hash)
        .map_or(0, |coin| coin.amount);
    println!("spend_bundle_id={bundle_id}");
    println!(
        "submitted_height={submitted_height} simulator_height_after_accept={}",
        sim.height()
    );
    println!("merchant_output_mojos={merchant_amount} user_output_mojos={user_amount}");
    println!("final_state={:?}", store.load_channel(channel_id)?.state);
    println!("SCENARIO_A=PASS\n");
    Ok(())
}

fn run_refund_demo() -> Result<()> {
    println!("SCENARIO B: NO-VOUCHER USER REFUND");
    let mut sim = Simulator::new();
    let [user, hub] = BlsPair::range_with_seed::<2>(800);
    let args = ChannelArgs::new(
        user.pk,
        hub.pk,
        user.puzzle_hash,
        TESTNET11_CONSTANTS.genesis_challenge,
        CLAIM_BEFORE_HEIGHT,
        REFUND_HEIGHT,
    )?;
    let (channel_puzzle_hash, _) = puzzle_reveal(&args)?;
    let funding_coin = sim.new_coin(channel_puzzle_hash, FUNDING_AMOUNT);
    let funding_coin_id = funding_coin.coin_id();
    let channel_id = channel_id(args.genesis_challenge, funding_coin_id);
    let mut store = ChannelStore::open_in_memory()?;
    store.create_channel(channel_id)?;
    store.mark_refundable(channel_id)?;
    advance_to(&mut sim, REFUND_HEIGHT as u32);
    let submitted_height = sim.height();

    let bundle = build_refund_bundle(
        funding_coin,
        &args,
        &user.sk,
        TESTNET11_CONSTANTS.agg_sig_me_additional_data,
    )?;
    let bundle_id = bundle.name();
    track_refund_submission(&mut store, channel_id)?;
    sim.new_transaction(bundle)?;
    let children = confirmed_children(&sim, funding_coin_id);
    confirm_refund(
        &mut store,
        channel_id,
        funding_coin_id,
        user.puzzle_hash,
        &children,
    )?;

    println!("funding_coin_id={funding_coin_id}");
    println!("voucher=NONE");
    println!("spend_bundle_id={bundle_id}");
    println!(
        "submitted_height={submitted_height} simulator_height_after_accept={}",
        sim.height()
    );
    println!("user_refund_mojos={}", children[0].amount);
    println!("final_state={:?}", store.load_channel(channel_id)?.state);
    println!("SCENARIO_B=PASS");
    Ok(())
}

fn main() -> Result<()> {
    println!("WALL-HUB DAY 7 MVP DEMO\n");
    run_claim_demo()?;
    run_refund_demo()?;
    println!("DAY_7_DEMO=PASS");
    Ok(())
}
