use anyhow::{Context, Result, ensure};
use chia_bls::PublicKey;
use chia_protocol::{Bytes, Bytes32, Coin, CoinSpend, Program};
use clvm_traits::ToClvm;
use clvm_utils::{CurriedProgram, tree_hash};
use clvmr::{
    Allocator,
    serde::{node_from_bytes, node_to_bytes},
};
use sha2::{Digest, Sha256};

mod chain;
mod api;
mod offchain;
mod service;
mod settlement;
mod state_store;
mod v2;
mod wallet_connect;
mod noise_session;

#[cfg(test)]
mod day6_tests;
#[cfg(test)]
mod hardening_tests;
#[cfg(test)]
mod offchain_tests;
#[cfg(test)]
mod settlement_tests;
#[cfg(test)]
mod state_store_tests;

pub use chain::*;
pub use api::*;
pub use offchain::*;
pub use service::*;
pub use settlement::*;
pub use state_store::*;
pub use v2::*;
pub use wallet_connect::*;
pub use noise_session::*;

pub const FUNDING_AMOUNT: u64 = 10;
pub const MERCHANT_AMOUNT: u64 = 1;
pub const USER_REMAINDER: u64 = 9;
pub const MIN_CLAIM_WINDOW_BLOCKS: u64 = 20;
pub const MAX_PROTOCOL_U64: u64 = i64::MAX as u64;

const CHANNEL_DOMAIN: &[u8] = b"WALL_HUB_CHANNEL_V1";
const INVOICE_DOMAIN: &[u8] = b"WALL_HUB_INVOICE_V1";
const SETTLEMENT_DOMAIN: &[u8] = b"WALL_HUB_SETTLEMENT_V1";
const REFUND_DOMAIN: &[u8] = b"WALL_HUB_REFUND_V1";
const PROTOCOL_VERSION: [u8; 2] = 1_u16.to_be_bytes();
const STATE_NUMBER: [u8; 8] = 1_u64.to_be_bytes();
const FEE_POLICY: [u8; 1] = [0];

#[derive(Debug, Clone, PartialEq, Eq, ToClvm)]
#[clvm(curry)]
pub struct ChannelArgs {
    pub user_public_key: PublicKey,
    pub hub_public_key: PublicKey,
    pub user_puzzle_hash: Bytes32,
    pub genesis_challenge: Bytes32,
    pub claim_before_height: Bytes,
    pub refund_height: Bytes,
}

impl ChannelArgs {
    pub fn new(
        user_public_key: PublicKey,
        hub_public_key: PublicKey,
        user_puzzle_hash: Bytes32,
        genesis_challenge: Bytes32,
        claim_before_height: u64,
        refund_height: u64,
    ) -> Result<Self> {
        ensure!(
            claim_before_height <= MAX_PROTOCOL_U64 && refund_height <= MAX_PROTOCOL_U64,
            "protocol heights must fit signed SQLite integers"
        );
        ensure!(
            claim_before_height
                .checked_add(1)
                .is_some_and(|height| height == refund_height),
            "refund height must be exactly one block after claim cutoff"
        );

        Ok(Self {
            user_public_key,
            hub_public_key,
            user_puzzle_hash,
            genesis_challenge,
            claim_before_height: fixed_u64(claim_before_height),
            refund_height: fixed_u64(refund_height),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, ToClvm)]
#[clvm(list)]
pub struct ChannelSolution {
    pub branch: u8,
    pub funding_coin_id: Bytes32,
    pub funding_amount: Bytes,
    pub user_remaining_amount: Bytes,
    pub invoice_hash: Bytes32,
    pub order_id: Bytes32,
    pub merchant_puzzle_hash: Bytes32,
    pub nonce: Bytes32,
    pub payment_expiry_height: Bytes,
}

impl ChannelSolution {
    pub fn claim(
        funding_coin_id: Bytes32,
        invoice_hash: Bytes32,
        order_id: Bytes32,
        merchant_puzzle_hash: Bytes32,
        nonce: Bytes32,
        payment_expiry_height: u64,
    ) -> Self {
        Self::claim_for_funding_amount(
            funding_coin_id,
            invoice_hash,
            order_id,
            merchant_puzzle_hash,
            nonce,
            payment_expiry_height,
            FUNDING_AMOUNT,
        )
        .expect("default funding amount is valid")
    }

    pub fn claim_for_funding_amount(
        funding_coin_id: Bytes32,
        invoice_hash: Bytes32,
        order_id: Bytes32,
        merchant_puzzle_hash: Bytes32,
        nonce: Bytes32,
        payment_expiry_height: u64,
        funding_amount: u64,
    ) -> Result<Self> {
        ensure!(
            funding_amount <= MAX_PROTOCOL_U64,
            "funding amount must fit signed SQLite integers"
        );
        ensure!(
            funding_amount > MERCHANT_AMOUNT,
            "funding amount must be greater than merchant amount"
        );
        let user_remaining_amount = funding_amount - MERCHANT_AMOUNT;
        Ok(Self {
            branch: 1,
            funding_coin_id,
            funding_amount: fixed_u64(funding_amount),
            user_remaining_amount: fixed_u64(user_remaining_amount),
            invoice_hash,
            order_id,
            merchant_puzzle_hash,
            nonce,
            payment_expiry_height: fixed_u64(payment_expiry_height),
        })
    }

    pub fn refund(funding_coin_id: Bytes32) -> Self {
        Self::refund_for_funding_amount(funding_coin_id, FUNDING_AMOUNT)
            .expect("default funding amount is valid")
    }

    pub fn refund_for_funding_amount(funding_coin_id: Bytes32, funding_amount: u64) -> Result<Self> {
        ensure!(
            funding_amount <= MAX_PROTOCOL_U64,
            "funding amount must fit signed SQLite integers"
        );
        Ok(Self {
            branch: 2,
            funding_coin_id,
            funding_amount: fixed_u64(funding_amount),
            user_remaining_amount: Bytes::new(Vec::new()),
            invoice_hash: Bytes32::default(),
            order_id: Bytes32::default(),
            merchant_puzzle_hash: Bytes32::default(),
            nonce: Bytes32::default(),
            payment_expiry_height: Bytes::new(Vec::new()),
        })
    }
}

pub fn channel_id(genesis_challenge: Bytes32, funding_coin_id: Bytes32) -> Bytes32 {
    hash_parts(&[
        CHANNEL_DOMAIN,
        genesis_challenge.as_ref(),
        funding_coin_id.as_ref(),
    ])
}

pub fn settlement_hash(args: &ChannelArgs, solution: &ChannelSolution) -> Bytes32 {
    hash_parts(&[
        SETTLEMENT_DOMAIN,
        &PROTOCOL_VERSION,
        args.genesis_challenge.as_ref(),
        solution.funding_coin_id.as_ref(),
        channel_id(args.genesis_challenge, solution.funding_coin_id).as_ref(),
        &STATE_NUMBER,
        solution.invoice_hash.as_ref(),
        solution.order_id.as_ref(),
        solution.merchant_puzzle_hash.as_ref(),
        &MERCHANT_AMOUNT.to_be_bytes(),
        args.user_puzzle_hash.as_ref(),
        solution.user_remaining_amount.as_ref(),
        solution.nonce.as_ref(),
        solution.payment_expiry_height.as_ref(),
        args.claim_before_height.as_ref(),
        args.refund_height.as_ref(),
        &FEE_POLICY,
    ])
}

pub fn refund_hash(args: &ChannelArgs, funding_coin_id: Bytes32) -> Bytes32 {
    refund_hash_for_funding_amount(args, funding_coin_id, FUNDING_AMOUNT)
}

pub fn refund_hash_for_funding_amount(
    args: &ChannelArgs,
    funding_coin_id: Bytes32,
    funding_amount: u64,
) -> Bytes32 {
    hash_parts(&[
        REFUND_DOMAIN,
        &PROTOCOL_VERSION,
        args.genesis_challenge.as_ref(),
        funding_coin_id.as_ref(),
        channel_id(args.genesis_challenge, funding_coin_id).as_ref(),
        args.user_puzzle_hash.as_ref(),
        &funding_amount.to_be_bytes(),
        args.refund_height.as_ref(),
        &FEE_POLICY,
    ])
}

pub fn puzzle_reveal(args: &ChannelArgs) -> Result<(Bytes32, Program)> {
    let mut allocator = Allocator::new();
    let module_bytes = hex::decode(include_str!("../puzzles/wall_hub_channel_v1.clsp.hex").trim())
        .context("invalid compiled puzzle hex")?;
    let module =
        node_from_bytes(&mut allocator, &module_bytes).context("invalid compiled CLVM module")?;
    let puzzle = CurriedProgram {
        program: module,
        args,
    }
    .to_clvm(&mut allocator)
    .context("failed to curry channel arguments")?;
    let puzzle_hash = Bytes32::from(tree_hash(&allocator, puzzle));
    let program = Program::from(
        node_to_bytes(&allocator, puzzle).context("failed to serialize channel puzzle")?,
    );
    Ok((puzzle_hash, program))
}

pub fn coin_spend(coin: Coin, args: &ChannelArgs, solution: &ChannelSolution) -> Result<CoinSpend> {
    let (puzzle_hash, puzzle_reveal) = puzzle_reveal(args)?;
    ensure!(
        coin.puzzle_hash == puzzle_hash,
        "coin does not use this channel puzzle"
    );

    let mut allocator = Allocator::new();
    let solution_node = solution
        .to_clvm(&mut allocator)
        .context("failed to encode channel solution")?;
    let solution = Program::from(
        node_to_bytes(&allocator, solution_node).context("failed to serialize solution")?,
    );
    Ok(CoinSpend::new(coin, puzzle_reveal, solution))
}

fn fixed_u64(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}

fn hash_parts(parts: &[&[u8]]) -> Bytes32 {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Bytes32::from(digest)
}

#[cfg(test)]
mod tests {
    use chia_consensus::validation_error::ErrorCode;
    use chia_protocol::{Bytes32, Coin, SpendBundle};
    use chia_sdk_signer::{AggSigConstants, RequiredSignature};
    use chia_sdk_test::{BlsPair, Simulator, SimulatorError, sign_transaction};
    use chia_sdk_types::TESTNET11_CONSTANTS;
    use clvmr::Allocator;

    use super::*;

    const CLAIM_BEFORE_HEIGHT: u64 = 25;
    const REFUND_HEIGHT: u64 = 26;
    const PAYMENT_EXPIRY_HEIGHT: u64 = 5;

    struct TestChannel {
        user: BlsPair,
        hub: BlsPair,
        merchant: BlsPair,
        args: ChannelArgs,
        coin: Coin,
        claim_solution: ChannelSolution,
    }

    fn setup(sim: &mut Simulator) -> TestChannel {
        let [user, hub, merchant] = BlsPair::range_with_seed::<3>(100);
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
        let claim_solution = ChannelSolution::claim(
            coin.coin_id(),
            Bytes32::from([0x31; 32]),
            Bytes32::from([0x32; 32]),
            merchant.puzzle_hash,
            Bytes32::from([0x33; 32]),
            PAYMENT_EXPIRY_HEIGHT,
        );
        TestChannel {
            user,
            hub,
            merchant,
            args,
            coin,
            claim_solution,
        }
    }

    fn signed_claim(channel: &TestChannel) -> SpendBundle {
        let spend = coin_spend(channel.coin, &channel.args, &channel.claim_solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            &[channel.user.sk.clone(), channel.hub.sk.clone()],
        )
        .unwrap();
        SpendBundle::new(vec![spend], signature)
    }

    fn signed_refund(channel: &TestChannel) -> SpendBundle {
        let solution = ChannelSolution::refund(channel.coin.coin_id());
        let spend = coin_spend(channel.coin, &channel.args, &solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            std::slice::from_ref(&channel.user.sk),
        )
        .unwrap();
        SpendBundle::new(vec![spend], signature)
    }

    fn advance_to(sim: &mut Simulator, height: u32) {
        while sim.height() < height {
            sim.create_block();
        }
    }

    #[test]
    fn claim_conditions_match_rust_commitment() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let spend = coin_spend(channel.coin, &channel.args, &channel.claim_solution).unwrap();
        let mut allocator = Allocator::new();
        let required = RequiredSignature::from_coin_spends(
            &mut allocator,
            std::slice::from_ref(&spend),
            &AggSigConstants::new(TESTNET11_CONSTANTS.agg_sig_me_additional_data),
        )
        .unwrap();

        assert_eq!(required.len(), 2);
        let expected_hash = settlement_hash(&channel.args, &channel.claim_solution);
        for signature in required {
            let RequiredSignature::Bls(signature) = signature else {
                panic!("expected BLS signature");
            };
            assert_eq!(signature.raw_message.as_ref(), expected_hash.as_ref());
        }
    }

    #[test]
    fn merchant_submits_claim_without_user_or_hub_keys() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        assert_eq!(channel.coin.amount, FUNDING_AMOUNT);

        let merchant_puzzle_hash = channel.merchant.puzzle_hash;
        let user_puzzle_hash = channel.user.puzzle_hash;
        let funding_coin_id = channel.coin.coin_id();
        let signature = signed_claim(&channel).aggregated_signature;
        let coin = channel.coin;
        let args = channel.args.clone();
        let claim_solution = channel.claim_solution.clone();

        drop(channel);
        let merchant_constructed_spend = coin_spend(coin, &args, &claim_solution).unwrap();
        sim.new_transaction(SpendBundle::new(
            vec![merchant_constructed_spend],
            signature,
        ))
        .unwrap();

        let children = sim.children(funding_coin_id);
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|state| {
            state.coin.puzzle_hash == merchant_puzzle_hash && state.coin.amount == MERCHANT_AMOUNT
        }));
        assert!(children.iter().any(|state| {
            state.coin.puzzle_hash == user_puzzle_hash && state.coin.amount == USER_REMAINDER
        }));
        assert_eq!(
            children.iter().map(|state| state.coin.amount).sum::<u64>(),
            FUNDING_AMOUNT
        );
    }

    #[test]
    fn modified_merchant_output_invalidates_signature() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let valid_bundle = signed_claim(&channel);

        let mut modified = channel.claim_solution.clone();
        modified.merchant_puzzle_hash = Bytes32::from([0x99; 32]);
        let modified_spend = coin_spend(channel.coin, &channel.args, &modified).unwrap();
        let result = sim.new_transaction(SpendBundle::new(
            vec![modified_spend],
            valid_bundle.aggregated_signature,
        ));

        assert!(matches!(
            result,
            Err(SimulatorError::Validation(ErrorCode::BadAggregateSignature))
        ));
    }

    #[test]
    fn curried_destination_and_heights_cannot_be_changed() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let original_hash = settlement_hash(&channel.args, &channel.claim_solution);

        let mut changed_destination = channel.args.clone();
        changed_destination.user_puzzle_hash = Bytes32::from([0x98; 32]);
        assert_ne!(
            settlement_hash(&changed_destination, &channel.claim_solution),
            original_hash
        );
        assert!(coin_spend(channel.coin, &changed_destination, &channel.claim_solution).is_err());

        let mut changed_heights = channel.args.clone();
        changed_heights.claim_before_height = fixed_u64(CLAIM_BEFORE_HEIGHT + 1);
        changed_heights.refund_height = fixed_u64(REFUND_HEIGHT + 1);
        assert_ne!(
            settlement_hash(&changed_heights, &channel.claim_solution),
            original_hash
        );
        assert!(coin_spend(channel.coin, &changed_heights, &channel.claim_solution).is_err());
    }

    #[test]
    fn signature_cannot_replay_on_another_funding_coin() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let valid_bundle = signed_claim(&channel);
        let second_coin = sim.new_coin(channel.coin.puzzle_hash, FUNDING_AMOUNT);
        let mut replay_solution = channel.claim_solution.clone();
        replay_solution.funding_coin_id = second_coin.coin_id();
        let replay_spend = coin_spend(second_coin, &channel.args, &replay_solution).unwrap();

        let result = sim.new_transaction(SpendBundle::new(
            vec![replay_spend],
            valid_bundle.aggregated_signature,
        ));
        assert!(matches!(
            result,
            Err(SimulatorError::Validation(ErrorCode::BadAggregateSignature))
        ));
    }

    #[test]
    fn funding_amount_is_exact() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let wrong_coin = sim.new_coin(channel.coin.puzzle_hash, FUNDING_AMOUNT - 1);
        let mut solution = channel.claim_solution.clone();
        solution.funding_coin_id = wrong_coin.coin_id();
        let spend = coin_spend(wrong_coin, &channel.args, &solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            &[channel.user.sk.clone(), channel.hub.sk.clone()],
        )
        .unwrap();

        let result = sim.new_transaction(SpendBundle::new(vec![spend], signature));
        assert!(matches!(
            result,
            Err(SimulatorError::Validation(ErrorCode::AssertMyAmountFailed))
        ));
    }

    #[test]
    fn custom_funding_amount_is_conserved() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let funding_amount = 17;
        let coin = sim.new_coin(channel.coin.puzzle_hash, funding_amount);
        let solution = ChannelSolution::claim_for_funding_amount(
            coin.coin_id(),
            Bytes32::from([0x31; 32]),
            Bytes32::from([0x32; 32]),
            channel.merchant.puzzle_hash,
            Bytes32::from([0x33; 32]),
            PAYMENT_EXPIRY_HEIGHT,
            funding_amount,
        )
        .unwrap();
        let spend = coin_spend(coin, &channel.args, &solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            &[channel.user.sk.clone(), channel.hub.sk.clone()],
        )
        .unwrap();

        sim.new_transaction(SpendBundle::new(vec![spend], signature))
            .unwrap();

        let children = sim.children(coin.coin_id());
        assert!(children.iter().any(|state| {
            state.coin.puzzle_hash == channel.merchant.puzzle_hash
                && state.coin.amount == MERCHANT_AMOUNT
        }));
        assert!(children.iter().any(|state| {
            state.coin.puzzle_hash == channel.user.puzzle_hash
                && state.coin.amount == funding_amount - MERCHANT_AMOUNT
        }));
    }

    #[test]
    fn claim_cutoff_is_inclusive_and_then_expires() {
        let mut at_cutoff = Simulator::new();
        let channel = setup(&mut at_cutoff);
        let bundle = signed_claim(&channel);
        advance_to(&mut at_cutoff, CLAIM_BEFORE_HEIGHT as u32);
        at_cutoff.new_transaction(bundle).unwrap();

        let mut after_cutoff = Simulator::new();
        let channel = setup(&mut after_cutoff);
        let bundle = signed_claim(&channel);
        advance_to(&mut after_cutoff, REFUND_HEIGHT as u32);
        let result = after_cutoff.new_transaction(bundle);
        assert!(matches!(
            result,
            Err(SimulatorError::Validation(
                ErrorCode::AssertBeforeHeightAbsoluteFailed
            ))
        ));
    }

    #[test]
    fn refund_starts_after_claim_cutoff() {
        let mut before_refund = Simulator::new();
        let channel = setup(&mut before_refund);
        let bundle = signed_refund(&channel);
        advance_to(&mut before_refund, CLAIM_BEFORE_HEIGHT as u32);
        let result = before_refund.new_transaction(bundle);
        assert!(matches!(
            result,
            Err(SimulatorError::Validation(
                ErrorCode::AssertHeightAbsoluteFailed
            ))
        ));

        let mut at_refund = Simulator::new();
        let channel = setup(&mut at_refund);
        let user_puzzle_hash = channel.user.puzzle_hash;
        let funding_coin_id = channel.coin.coin_id();
        let bundle = signed_refund(&channel);
        advance_to(&mut at_refund, REFUND_HEIGHT as u32);
        at_refund.new_transaction(bundle).unwrap();
        let children = at_refund.children(funding_coin_id);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].coin.puzzle_hash, user_puzzle_hash);
        assert_eq!(children[0].coin.amount, FUNDING_AMOUNT);
    }

    #[test]
    fn same_voucher_cannot_be_claimed_twice() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let bundle = signed_claim(&channel);
        sim.new_transaction(bundle.clone()).unwrap();
        let result = sim.new_transaction(bundle);
        assert!(matches!(
            result,
            Err(SimulatorError::Validation(ErrorCode::DoubleSpend))
        ));
    }

    #[test]
    fn hub_signature_is_required_before_voucher_issuance() {
        let mut sim = Simulator::new();
        let channel = setup(&mut sim);
        let spend = coin_spend(channel.coin, &channel.args, &channel.claim_solution).unwrap();
        let result = sign_transaction(std::slice::from_ref(&spend), &[channel.user.sk]);
        assert!(matches!(result, Err(SimulatorError::MissingKey)));
    }
}
