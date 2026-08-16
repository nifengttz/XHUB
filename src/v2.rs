use anyhow::{Context, Result, ensure};
use chia_bls::PublicKey;
use chia_protocol::{Bytes, Bytes32, Coin, CoinSpend, Program};
use clvm_traits::ToClvm;
use clvm_utils::{CurriedProgram, tree_hash};
use clvmr::{
    Allocator,
    serde::{node_from_bytes, node_to_bytes},
};

use crate::{MAX_PROTOCOL_U64, hash_parts};

pub const CHANNEL_PROTOCOL_VERSION: u16 = 2;
const CHANNEL_DOMAIN: &[u8] = b"WALL_HUB_CHANNEL_V2";
const TERMS_DOMAIN: &[u8] = b"WALL_HUB_TERMS_V2";
const SETTLEMENT_DOMAIN: &[u8] = b"WALL_HUB_SETTLEMENT_V2";
const REFUND_DOMAIN: &[u8] = b"WALL_HUB_REFUND_V2";
const FEE_POLICY: [u8; 1] = [0];

#[derive(Debug, Clone, PartialEq, Eq, ToClvm)]
#[clvm(curry)]
pub struct ChannelTermsV2 {
    pub user_public_key: PublicKey,
    pub hub_public_key: PublicKey,
    pub user_puzzle_hash: Bytes32,
    pub genesis_challenge: Bytes32,
    pub agg_sig_me_additional_data: Bytes32,
    pub funding_amount: Bytes,
    pub claim_before_height: Bytes,
    pub refund_height: Bytes,
}

impl ChannelTermsV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_public_key: PublicKey,
        hub_public_key: PublicKey,
        user_puzzle_hash: Bytes32,
        genesis_challenge: Bytes32,
        agg_sig_me_additional_data: Bytes32,
        funding_amount: u64,
        claim_before_height: u64,
        refund_height: u64,
    ) -> Result<Self> {
        ensure!(
            funding_amount > 0 && funding_amount <= MAX_PROTOCOL_U64,
            "V2 funding amount is out of range"
        );
        ensure!(
            claim_before_height <= MAX_PROTOCOL_U64 && refund_height <= MAX_PROTOCOL_U64,
            "V2 heights are out of range"
        );
        ensure!(
            refund_height > claim_before_height,
            "V2 refund height must be after claim cutoff"
        );
        Ok(Self {
            user_public_key,
            hub_public_key,
            user_puzzle_hash,
            genesis_challenge,
            agg_sig_me_additional_data,
            funding_amount: fixed_u64(funding_amount),
            claim_before_height: fixed_u64(claim_before_height),
            refund_height: fixed_u64(refund_height),
        })
    }

    pub fn funding_amount_u64(&self) -> Result<u64> {
        read_fixed_u64(&self.funding_amount, "funding_amount")
    }

    pub fn channel_terms_hash(&self) -> Bytes32 {
        let hub_public_key = self.hub_public_key.to_bytes();
        let user_public_key = self.user_public_key.to_bytes();
        hash_parts(&[
            TERMS_DOMAIN,
            &CHANNEL_PROTOCOL_VERSION.to_be_bytes(),
            self.genesis_challenge.as_ref(),
            self.agg_sig_me_additional_data.as_ref(),
            &hub_public_key,
            &user_public_key,
            self.user_puzzle_hash.as_ref(),
            self.funding_amount.as_ref(),
            self.claim_before_height.as_ref(),
            self.refund_height.as_ref(),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, ToClvm)]
#[clvm(list)]
pub struct ChannelSolutionV2 {
    pub branch: u8,
    pub funding_coin_id: Bytes32,
    pub merchant_puzzle_hash: Bytes32,
    pub settlement_amount: Bytes,
    pub user_remainder: Bytes,
    pub settlement_nonce: Bytes32,
}

impl ChannelSolutionV2 {
    pub fn claim(
        funding_coin_id: Bytes32,
        merchant_puzzle_hash: Bytes32,
        settlement_amount: u64,
        user_remainder: u64,
        settlement_nonce: Bytes32,
        funding_amount: u64,
    ) -> Result<Self> {
        ensure!(
            settlement_amount > 0 && user_remainder > 0,
            "V2 settlement outputs must be positive"
        );
        ensure!(
            settlement_amount.checked_add(user_remainder) == Some(funding_amount),
            "V2 settlement must conserve funding amount"
        );
        Ok(Self {
            branch: 1,
            funding_coin_id,
            merchant_puzzle_hash,
            settlement_amount: fixed_u64(settlement_amount),
            user_remainder: fixed_u64(user_remainder),
            settlement_nonce,
        })
    }

    pub fn refund(funding_coin_id: Bytes32) -> Self {
        Self {
            branch: 2,
            funding_coin_id,
            merchant_puzzle_hash: Bytes32::default(),
            settlement_amount: Bytes::new(Vec::new()),
            user_remainder: Bytes::new(Vec::new()),
            settlement_nonce: Bytes32::default(),
        }
    }
}

pub fn channel_id_v2(genesis_challenge: Bytes32, funding_coin_id: Bytes32) -> Bytes32 {
    hash_parts(&[
        CHANNEL_DOMAIN,
        genesis_challenge.as_ref(),
        funding_coin_id.as_ref(),
    ])
}

pub fn settlement_hash_v2(terms: &ChannelTermsV2, solution: &ChannelSolutionV2) -> Bytes32 {
    hash_parts(&[
        SETTLEMENT_DOMAIN,
        &CHANNEL_PROTOCOL_VERSION.to_be_bytes(),
        terms.genesis_challenge.as_ref(),
        solution.funding_coin_id.as_ref(),
        channel_id_v2(terms.genesis_challenge, solution.funding_coin_id).as_ref(),
        terms.channel_terms_hash().as_ref(),
        solution.merchant_puzzle_hash.as_ref(),
        solution.settlement_amount.as_ref(),
        terms.user_puzzle_hash.as_ref(),
        solution.user_remainder.as_ref(),
        solution.settlement_nonce.as_ref(),
        terms.claim_before_height.as_ref(),
        terms.refund_height.as_ref(),
        &FEE_POLICY,
    ])
}

pub fn refund_hash_v2(terms: &ChannelTermsV2, funding_coin_id: Bytes32) -> Bytes32 {
    hash_parts(&[
        REFUND_DOMAIN,
        &CHANNEL_PROTOCOL_VERSION.to_be_bytes(),
        terms.genesis_challenge.as_ref(),
        funding_coin_id.as_ref(),
        channel_id_v2(terms.genesis_challenge, funding_coin_id).as_ref(),
        terms.channel_terms_hash().as_ref(),
        terms.user_puzzle_hash.as_ref(),
        terms.funding_amount.as_ref(),
        terms.refund_height.as_ref(),
        &FEE_POLICY,
    ])
}

pub fn puzzle_reveal_v2(terms: &ChannelTermsV2) -> Result<(Bytes32, Program)> {
    let mut allocator = Allocator::new();
    let module_bytes = hex::decode(include_str!("../puzzles/wall_hub_channel_v2.clsp.hex").trim())
        .context("invalid V2 compiled puzzle hex")?;
    let module =
        node_from_bytes(&mut allocator, &module_bytes).context("invalid V2 CLVM module")?;
    let puzzle = CurriedProgram {
        program: module,
        args: terms,
    }
    .to_clvm(&mut allocator)
    .context("failed to curry V2 channel terms")?;
    let puzzle_hash = Bytes32::from(tree_hash(&allocator, puzzle));
    let program = Program::from(
        node_to_bytes(&allocator, puzzle).context("failed to serialize V2 channel puzzle")?,
    );
    Ok((puzzle_hash, program))
}

pub fn coin_spend_v2(
    coin: Coin,
    terms: &ChannelTermsV2,
    solution: &ChannelSolutionV2,
) -> Result<CoinSpend> {
    let (puzzle_hash, puzzle_reveal) = puzzle_reveal_v2(terms)?;
    ensure!(
        coin.puzzle_hash == puzzle_hash,
        "coin does not use this V2 channel puzzle"
    );
    let mut allocator = Allocator::new();
    let node = solution
        .to_clvm(&mut allocator)
        .context("failed to encode V2 solution")?;
    let solution =
        Program::from(node_to_bytes(&allocator, node).context("failed to serialize V2 solution")?);
    Ok(CoinSpend::new(coin, puzzle_reveal, solution))
}

fn fixed_u64(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}
fn read_fixed_u64(value: &[u8], field: &str) -> Result<u64> {
    ensure!(value.len() == 8, "{field} must be an 8-byte u64");
    Ok(u64::from_be_bytes(
        value.try_into().expect("validated length"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chia_consensus::validation_error::ErrorCode;
    use chia_protocol::SpendBundle;
    use chia_sdk_test::{BlsPair, Simulator, SimulatorError, sign_transaction};
    use chia_sdk_types::MAINNET_CONSTANTS;

    fn setup(
        sim: &mut Simulator,
    ) -> (
        BlsPair,
        BlsPair,
        BlsPair,
        ChannelTermsV2,
        Coin,
        ChannelSolutionV2,
    ) {
        let [user, hub, merchant] = BlsPair::range_with_seed::<3>(230);
        let terms = ChannelTermsV2::new(
            user.pk,
            hub.pk,
            user.puzzle_hash,
            MAINNET_CONSTANTS.genesis_challenge,
            MAINNET_CONSTANTS.agg_sig_me_additional_data,
            1_000_000,
            25,
            26,
        )
        .unwrap();
        let (puzzle_hash, _) = puzzle_reveal_v2(&terms).unwrap();
        let coin = sim.new_coin(puzzle_hash, 1_000_000);
        let solution = ChannelSolutionV2::claim(
            coin.coin_id(),
            merchant.puzzle_hash,
            250_000,
            750_000,
            Bytes32::from([0x77; 32]),
            1_000_000,
        )
        .unwrap();
        (user, hub, merchant, terms, coin, solution)
    }

    #[test]
    fn claim_conserves_variable_funding_and_requires_both_keys() {
        let mut sim = Simulator::new();
        let (user, hub, merchant, terms, coin, solution) = setup(&mut sim);
        let spend = coin_spend_v2(coin, &terms, &solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            &[user.sk.clone(), hub.sk.clone()],
        )
        .unwrap();
        sim.new_transaction(SpendBundle::new(vec![spend], signature))
            .unwrap();
        let children = sim.children(coin.coin_id());
        assert!(
            children
                .iter()
                .any(|child| child.coin.puzzle_hash == merchant.puzzle_hash
                    && child.coin.amount == 250_000)
        );
        assert_eq!(
            children.iter().map(|child| child.coin.amount).sum::<u64>(),
            1_000_000
        );
    }

    #[test]
    fn refund_requires_only_user_after_refund_height() {
        let mut sim = Simulator::new();
        let (user, _, _, terms, coin, _) = setup(&mut sim);
        let spend =
            coin_spend_v2(coin, &terms, &ChannelSolutionV2::refund(coin.coin_id())).unwrap();
        let signature =
            sign_transaction(std::slice::from_ref(&spend), std::slice::from_ref(&user.sk)).unwrap();
        while sim.height() < 26 {
            sim.create_block();
        }
        sim.new_transaction(SpendBundle::new(vec![spend], signature))
            .unwrap();
        let children = sim.children(coin.coin_id());
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].coin.puzzle_hash, user.puzzle_hash);
        assert_eq!(children[0].coin.amount, 1_000_000);
    }

    #[test]
    fn altered_terms_or_nonconserving_solution_are_rejected() {
        let mut sim = Simulator::new();
        let (user, hub, _, terms, coin, solution) = setup(&mut sim);
        assert!(
            ChannelSolutionV2::claim(
                coin.coin_id(),
                solution.merchant_puzzle_hash,
                1,
                2,
                solution.settlement_nonce,
                1_000_000
            )
            .is_err()
        );
        let spend = coin_spend_v2(coin, &terms, &solution).unwrap();
        let signature = sign_transaction(
            std::slice::from_ref(&spend),
            &[user.sk.clone(), hub.sk.clone()],
        )
        .unwrap();
        let mut altered = terms.clone();
        altered.user_puzzle_hash = Bytes32::from([0x99; 32]);
        assert!(coin_spend_v2(coin, &altered, &solution).is_err());
        let mut changed_solution = solution.clone();
        changed_solution.merchant_puzzle_hash = Bytes32::from([0x98; 32]);
        let changed_spend = coin_spend_v2(coin, &terms, &changed_solution).unwrap();
        assert!(matches!(
            sim.new_transaction(SpendBundle::new(vec![changed_spend], signature)),
            Err(SimulatorError::Validation(ErrorCode::BadAggregateSignature))
        ));
    }

    #[test]
    fn deterministic_vector_matches_published_values() {
        let mut sim = Simulator::new();
        let (user, hub, merchant, terms, coin, solution) = setup(&mut sim);
        let (puzzle_hash, _) = puzzle_reveal_v2(&terms).unwrap();
        assert_eq!(
            hex::encode(user.pk.to_bytes()),
            "8c285a81f47f67fcd1b0e08bbd065f5c8c15caf4ff1fb969760c2013fb7d267962a56697a8c413aa75aae062b2fbca74"
        );
        assert_eq!(
            hex::encode(hub.pk.to_bytes()),
            "935a603d33e5f66ba9337c44d426ebca3819715515a59007ec65bf4c4d8e3b7d259e1952aa0ebab59947f0a6ff691202"
        );
        assert_eq!(
            hex::encode(merchant.puzzle_hash),
            "d40eae3807ae7a3c73f2e1a52890648a6ba0a69f5c537d5dd6bcf977ff5a57be"
        );
        assert_eq!(
            hex::encode(user.puzzle_hash),
            "a17d522869528be7fdc3263bef2724295d652ccfe31a059f19bf8e9fe67210b3"
        );
        assert_eq!(
            hex::encode(coin.coin_id()),
            "6d91591e1e1b1fc81a8ddad964cc6fd4b0e9afb8df8f0bf47f65e8eb2635fcf4"
        );
        assert_eq!(
            hex::encode(channel_id_v2(terms.genesis_challenge, coin.coin_id())),
            "640ee4c36e6ebe34bc5db65552c881f891cd228a397329add53c9adacc25b696"
        );
        assert_eq!(
            hex::encode(terms.channel_terms_hash()),
            "fd8c21597bebf7206ad13230c5b381f2f673a2a76ee60dae60e07b18e3f58623"
        );
        assert_eq!(
            hex::encode(settlement_hash_v2(&terms, &solution)),
            "c9b4aea65e8522cfbf80d154d7ad79baac5bfe5aac5b6cfebcc3b3fa2338ea05"
        );
        assert_eq!(
            hex::encode(refund_hash_v2(&terms, coin.coin_id())),
            "bd796b03ac123d0e66118bdc6747b6f0ea25b368b789897f7d371dc0beb84850"
        );
        assert_eq!(
            hex::encode(puzzle_hash),
            "443a52f2ada15456654eac806f185a9007f56212cdd8605c867abcf421317c9c"
        );
    }
}
