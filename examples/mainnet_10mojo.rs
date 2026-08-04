use std::{env, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, ensure};
use chia_bls::SecretKey;
use chia_protocol::{Bytes32, SpendBundle};
use chia_sdk_types::MAINNET_CONSTANTS;
use chia_sdk_utils::Address;
use chia_traits::Streamable;
use sha2::{Digest, Sha256};
use wall_hub_mvp::{
    ChannelArgs, ChannelSolution, ChiaNode, ChiaRpcConfig, FUNDING_AMOUNT, InvoiceFields,
    MerchantInvoice, PaymentIntent, PaymentVoucher, SettlementCommitment, build_claim_bundle,
    build_refund_bundle, puzzle_reveal,
};

const USER_SEED: [u8; 32] = [0x11; 32];
const HUB_SEED: [u8; 32] = [0x22; 32];
const FULL_NODE_URL: &str = "https://127.0.0.1:8555";
const CHIA_ROOT: &str = "D:\\chia\\mainnet";

fn mainnet_node() -> Result<ChiaNode> {
    let ssl = PathBuf::from(CHIA_ROOT).join("config\\ssl\\full_node");
    ChiaNode::connect(
        ChiaRpcConfig::FullNode {
            base_url: FULL_NODE_URL.to_string(),
            cert_path: ssl.join("private_full_node.crt"),
            key_path: ssl.join("private_full_node.key"),
        },
        MAINNET_CONSTANTS.genesis_challenge,
    )
    .map_err(Into::into)
}

fn address_puzzle_hash(value: &str) -> Result<Bytes32> {
    Address::decode(value)
        .context("invalid Chia address")?
        .expect_prefix("xch")
        .map_err(Into::into)
}

fn channel_args(user_address: &str, claim_before: u64, refund_height: u64) -> Result<ChannelArgs> {
    let user_key = SecretKey::from_seed(&USER_SEED);
    ChannelArgs::new(
        user_key.public_key(),
        SecretKey::from_seed(&HUB_SEED).public_key(),
        address_puzzle_hash(user_address)?,
        MAINNET_CONSTANTS.genesis_challenge,
        claim_before,
        refund_height,
    )
}

fn print_channel(
    user_address: &str,
    merchant_address: &str,
    claim_before: u64,
    refund_height: u64,
) -> Result<()> {
    let args = channel_args(user_address, claim_before, refund_height)?;
    let (puzzle_hash, _) = puzzle_reveal(&args)?;
    let channel_address = Address::new(puzzle_hash, "xch".to_string()).encode()?;
    println!("channel_address={channel_address}");
    println!("channel_puzzle_hash={puzzle_hash}");
    println!("user_address={user_address}");
    println!("merchant_address={merchant_address}");
    println!("funding_amount_mojos={FUNDING_AMOUNT}");
    println!("claim_before_height={claim_before}");
    println!("refund_height={refund_height}");
    Ok(())
}

fn hash_id(domain: &[u8], coin_id: Bytes32) -> Bytes32 {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(coin_id.as_ref());
    let digest: [u8; 32] = hasher.finalize().into();
    Bytes32::from(digest)
}

async fn find_coin(address: &str) -> Result<()> {
    let node = mainnet_node()?;
    let puzzle_hash = address_puzzle_hash(address)?;
    let records = node.get_unspent_coins(puzzle_hash, FUNDING_AMOUNT).await?;
    ensure!(
        !records.is_empty(),
        "no unspent coin of at least 10 mojo found"
    );
    for record in records {
        println!(
            "coin_id={} amount_mojos={} confirmed_height={}",
            record.coin.coin_id(),
            record.coin.amount,
            record.confirmed_block_index
        );
    }
    Ok(())
}

fn funding_id_from_bundle(transaction_file: &str, address: &str) -> Result<()> {
    let target = address_puzzle_hash(address)?;
    let bytes = fs::read(transaction_file).context("failed to read transaction file")?;
    let bundle = SpendBundle::from_bytes(&bytes).context("failed to decode wallet SpendBundle")?;
    let additions = bundle
        .additions()
        .map_err(|error| anyhow::anyhow!("failed to evaluate wallet SpendBundle: {error}"))?;
    let funding_coin = additions
        .into_iter()
        .find(|coin| coin.puzzle_hash == target && coin.amount == FUNDING_AMOUNT)
        .context("wallet transaction did not contain the requested 10 mojo output")?;
    println!("coin_id={}", funding_coin.coin_id());
    println!("amount_mojos={}", funding_coin.amount);
    println!("puzzle_hash={}", funding_coin.puzzle_hash);
    Ok(())
}

async fn claim(
    funding_coin_id: Bytes32,
    user_address: &str,
    merchant_address: &str,
    claim_before: u64,
    refund_height: u64,
    payment_expiry: u64,
) -> Result<()> {
    let node = mainnet_node()?;
    let status = node.status().await?;
    ensure!(status.synced, "mainnet full node is not synced");
    ensure!(
        u64::from(status.peak_height) <= claim_before,
        "claim cutoff has passed"
    );
    let record = node
        .get_coin(funding_coin_id)
        .await?
        .context("funding coin not found")?;
    ensure!(!record.spent, "funding coin is already spent");
    ensure!(
        record.coin.amount == FUNDING_AMOUNT,
        "funding coin is not exactly 10 mojo"
    );

    let user_key = SecretKey::from_seed(&USER_SEED);
    let hub_key = SecretKey::from_seed(&HUB_SEED);
    let args = channel_args(user_address, claim_before, refund_height)?;
    let merchant_puzzle_hash = address_puzzle_hash(merchant_address)?;
    let order_id = hash_id(b"WALL_HUB_MAINNET_ORDER_V1", funding_coin_id);
    let nonce = hash_id(b"WALL_HUB_MAINNET_NONCE_V1", funding_coin_id);
    let invoice = MerchantInvoice::issue(
        InvoiceFields::new(
            MAINNET_CONSTANTS.genesis_challenge,
            funding_coin_id,
            order_id,
            merchant_puzzle_hash,
            payment_expiry,
            nonce,
        ),
        &hub_key,
    );
    let solution = ChannelSolution::claim(
        funding_coin_id,
        invoice.invoice_hash,
        order_id,
        merchant_puzzle_hash,
        nonce,
        payment_expiry,
    );
    let commitment = SettlementCommitment::from_channel(&args, &solution)?;
    let intent = PaymentIntent::sign(
        commitment,
        &invoice,
        &args,
        &user_key,
        MAINNET_CONSTANTS.agg_sig_me_additional_data,
        u64::from(status.peak_height),
    )?;
    let voucher = PaymentVoucher::issue(
        intent,
        &invoice,
        &args,
        &hub_key,
        MAINNET_CONSTANTS.agg_sig_me_additional_data,
        u64::from(status.peak_height),
    )?;
    let bundle = build_claim_bundle(record.coin, &args, &voucher)?;
    let tx_id = node.broadcast(bundle).await?;
    println!("branch=CLAIM");
    println!("funding_coin_id={funding_coin_id}");
    println!("channel_id={}", voucher.intent.commitment.channel_id);
    println!("transaction_id={tx_id}");
    println!("fee_policy=zero_fee_compatibility_sample");
    println!("fee_mojos=0");
    println!("confirmation_depth=3");
    let observation = node
        .wait_for_confirmation(tx_id, funding_coin_id, 3, Duration::from_secs(20), 30)
        .await?;
    println!("confirmed_height={:?}", observation.confirmed_height);
    for child in observation.children {
        println!(
            "child_id={} amount_mojos={} puzzle_hash={} confirmed_height={}",
            child.coin.coin_id(),
            child.coin.amount,
            child.coin.puzzle_hash,
            child.confirmed_block_index
        );
    }
    Ok(())
}

async fn refund(
    funding_coin_id: Bytes32,
    user_address: &str,
    claim_before: u64,
    refund_height: u64,
) -> Result<()> {
    let node = mainnet_node()?;
    loop {
        let status = node.status().await?;
        if u64::from(status.peak_height) >= refund_height {
            break;
        }
        println!(
            "waiting_for_refund_height={refund_height} current_height={}",
            status.peak_height
        );
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
    let record = node
        .get_coin(funding_coin_id)
        .await?
        .context("funding coin not found")?;
    ensure!(!record.spent, "funding coin is already spent");
    ensure!(
        record.coin.amount == FUNDING_AMOUNT,
        "funding coin is not exactly 10 mojo"
    );
    let args = channel_args(user_address, claim_before, refund_height)?;
    let user_key = SecretKey::from_seed(&USER_SEED);
    let bundle = build_refund_bundle(
        record.coin,
        &args,
        &user_key,
        MAINNET_CONSTANTS.agg_sig_me_additional_data,
    )?;
    let tx_id = node.broadcast(bundle).await?;
    println!("branch=REFUND");
    println!("funding_coin_id={funding_coin_id}");
    println!("transaction_id={tx_id}");
    println!("fee_policy=zero_fee_compatibility_sample");
    println!("fee_mojos=0");
    println!("confirmation_depth=3");
    let observation = node
        .wait_for_confirmation(tx_id, funding_coin_id, 3, Duration::from_secs(20), 30)
        .await?;
    println!("confirmed_height={:?}", observation.confirmed_height);
    for child in observation.children {
        println!(
            "child_id={} amount_mojos={} puzzle_hash={} confirmed_height={}",
            child.coin.coin_id(),
            child.coin.amount,
            child.coin.puzzle_hash,
            child.confirmed_block_index
        );
    }
    Ok(())
}

fn parse_id(value: &str) -> Result<Bytes32> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).context("invalid coin id")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("coin id must be 32 bytes"))?;
    Ok(Bytes32::from(bytes))
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  prepare-claim <user_address> <merchant_address>\n  prepare-refund <user_address> <merchant_address>\n  find <channel_address>\n  funding-id <transaction_file> <channel_address>\n  claim <coin_id> <user_address> <merchant_address> <claim_before> <refund_height> <payment_expiry>\n  refund <coin_id> <user_address> <claim_before> <refund_height>"
    );
    std::process::exit(2);
}

async fn async_main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| usage());
    match command.as_str() {
        "prepare-claim" => {
            let user = args.next().unwrap_or_else(|| usage());
            let merchant = args.next().unwrap_or_else(|| usage());
            let node = mainnet_node()?;
            let peak = u64::from(node.status().await?.peak_height);
            print_channel(&user, &merchant, peak + 40, peak + 41)?;
            println!("payment_expiry_height={}", peak + 20);
        }
        "prepare-refund" => {
            let user = args.next().unwrap_or_else(|| usage());
            let merchant = args.next().unwrap_or_else(|| usage());
            let node = mainnet_node()?;
            let peak = u64::from(node.status().await?.peak_height);
            print_channel(&user, &merchant, peak + 1, peak + 2)?;
            println!("payment_expiry_height=0");
        }
        "find" => find_coin(&args.next().unwrap_or_else(|| usage())).await?,
        "funding-id" => funding_id_from_bundle(
            &args.next().unwrap_or_else(|| usage()),
            &args.next().unwrap_or_else(|| usage()),
        )?,
        "claim" => {
            let coin = parse_id(&args.next().unwrap_or_else(|| usage()))?;
            let user = args.next().unwrap_or_else(|| usage());
            let merchant = args.next().unwrap_or_else(|| usage());
            let claim_before = args.next().unwrap_or_else(|| usage()).parse()?;
            let refund_height = args.next().unwrap_or_else(|| usage()).parse()?;
            let payment_expiry = args.next().unwrap_or_else(|| usage()).parse()?;
            claim(
                coin,
                &user,
                &merchant,
                claim_before,
                refund_height,
                payment_expiry,
            )
            .await?;
        }
        "refund" => {
            let coin = parse_id(&args.next().unwrap_or_else(|| usage()))?;
            let user = args.next().unwrap_or_else(|| usage());
            let claim_before = args.next().unwrap_or_else(|| usage()).parse()?;
            let refund_height = args.next().unwrap_or_else(|| usage()).parse()?;
            refund(coin, &user, claim_before, refund_height).await?;
        }
        _ => usage(),
    }
    Ok(())
}

fn main() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}
