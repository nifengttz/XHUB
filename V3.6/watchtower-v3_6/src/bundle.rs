use std::collections::HashSet;

use chia_bls::{SecretKey, aggregate, sign};
use chia_consensus::{
    consensus_constants::TEST_CONSTANTS, flags::MEMPOOL_MODE,
    spendbundle_validation::validate_clvm_and_signature,
};
use chia_protocol::{Coin, CoinSpend, Program, SpendBundle};
use chia_sdk_types::{Mod, puzzles::P2DelegatedConditionsArgs};
use clvm_traits::ToClvm;
use clvm_utils::{CurriedProgram, tree_hash};
use clvmr::{Allocator, NodePtr, serde::node_to_bytes};
use serde::Serialize;
use xhub_protocol_v3_6::{Bytes32, RecoveryPackage, sha256_parts};
use xhub_puzzles_v3_6::{
    ChallengeSpendMaterial, ClosingCoinKind, challenge_spend_material,
    state_zero_challenge_spend_material,
};

const MAX_COST: u64 = 11_000_000_000;
pub const SPEND_BUNDLE_COMMITMENT_DOMAIN: &[u8] = b"XHUB_SPEND_BUNDLE_COMMITMENT_V3_6";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSnapshot {
    pub peak_height: u64,
    pub peak_header_hash: Bytes32,
    pub closing_coin_id: Bytes32,
    pub closing_coin: Coin,
    pub closing_birth_height: u64,
    pub closing_spent_height: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TestFeeSponsor {
    pub coin: Coin,
    pub secret_key: SecretKey,
    pub change_puzzle_hash: Bytes32,
    pub fee_mojo: u64,
}

#[derive(Debug, Clone)]
pub struct OfflineChallengeBundle {
    spend_bundle: SpendBundle,
    construction_snapshot: ChainSnapshot,
    report: OfflineBundleReport,
    commitment: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfflineBundleReport {
    pub schema: &'static str,
    pub closing_coin_id: String,
    pub next_closing_coin_id: String,
    pub fee_coin_id: String,
    pub fee_mojo: u64,
    pub removal_amount_mojo: u128,
    pub addition_amount_mojo: u128,
    pub cost: u64,
    pub consensus_conditions_verified: bool,
    pub aggregate_signature_verified: bool,
    pub spend_bundle_created: bool,
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

impl OfflineChallengeBundle {
    pub fn report(&self) -> &OfflineBundleReport {
        &self.report
    }

    pub fn commitment(&self) -> Bytes32 {
        self.commitment
    }

    pub fn validate_pre_broadcast_snapshot(&self, current: &ChainSnapshot) -> Result<(), String> {
        if current != &self.construction_snapshot {
            return Err("chain snapshot changed after offline bundle construction".into());
        }
        if current.closing_spent_height.is_some() {
            return Err("Closing Coin is already spent".into());
        }
        if current.peak_height >= self.report_deadline()? {
            return Err("challenge deadline has passed".into());
        }
        Ok(())
    }

    fn report_deadline(&self) -> Result<u64, String> {
        let mut allocator = Allocator::new();
        let (conditions, _) = chia_consensus::spendbundle_conditions::run_spendbundle(
            &mut allocator,
            &self.spend_bundle,
            MAX_COST,
            MEMPOOL_MODE,
            &TEST_CONSTANTS,
        )
        .map_err(|error| format!("consensus revalidation failed: {error:?}"))?;
        conditions
            .before_height_absolute
            .map(u64::from)
            .ok_or_else(|| "bundle omitted ASSERT_BEFORE_HEIGHT_ABSOLUTE".into())
    }
}

pub fn build_offline_challenge_bundle(
    current: Option<&RecoveryPackage>,
    latest: &RecoveryPackage,
    closing_coin_kind: ClosingCoinKind,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
    snapshot: ChainSnapshot,
    fee: &TestFeeSponsor,
) -> Result<OfflineChallengeBundle, String> {
    if snapshot.closing_spent_height.is_some() {
        return Err("Closing Coin is already spent".into());
    }
    if closing_coin_kind == ClosingCoinKind::Initial
        && snapshot.closing_birth_height != initial_birth_height
    {
        return Err("Initial Closing Coin birth height changed".into());
    }
    if snapshot.peak_height >= challenge_deadline_height {
        return Err("challenge deadline has passed".into());
    }
    let material = match current {
        Some(current) => challenge_spend_material(
            current,
            latest,
            closing_coin_kind,
            initial_birth_height,
            challenge_deadline_height,
        )?,
        None => {
            if closing_coin_kind != ClosingCoinKind::Initial {
                return Err("State 0 can only exist in an Initial Closing Coin".into());
            }
            state_zero_challenge_spend_material(
                latest,
                initial_birth_height,
                challenge_deadline_height,
            )?
        }
    };
    validate_closing_coin(&snapshot, &material, latest.funding_amount)?;
    if fee.fee_mojo == 0 || fee.fee_mojo >= fee.coin.amount {
        return Err("test fee must be positive and smaller than the fee Coin amount".into());
    }

    let closing_spend = CoinSpend::new(
        snapshot.closing_coin,
        material.puzzle_reveal,
        material.solution,
    );
    let fee_spend = fee_spend(fee)?;
    let mut spend_bundle = SpendBundle {
        coin_spends: vec![closing_spend, fee_spend],
        aggregated_signature: aggregate(&material.protocol_signatures),
    };
    validate_unique_removals(&spend_bundle)?;

    let mut allocator = Allocator::new();
    let (_, pairs) = chia_consensus::spendbundle_conditions::run_spendbundle(
        &mut allocator,
        &spend_bundle,
        MAX_COST,
        MEMPOOL_MODE,
        &TEST_CONSTANTS,
    )
    .map_err(|error| format!("consensus condition validation failed: {error:?}"))?;
    let fee_public_key = fee.secret_key.public_key();
    let fee_messages = pairs
        .iter()
        .filter(|(public_key, _)| public_key == &fee_public_key)
        .map(|(_, message)| message.as_slice())
        .collect::<Vec<_>>();
    if fee_messages.len() != 1 {
        return Err("test fee puzzle did not emit exactly one fee signature message".into());
    }
    let fee_signature = sign(&fee.secret_key, fee_messages[0]);
    spend_bundle.aggregated_signature = aggregate(
        material
            .protocol_signatures
            .iter()
            .chain(std::iter::once(&fee_signature)),
    );

    let (conditions, _) =
        validate_clvm_and_signature(&spend_bundle, MAX_COST, &TEST_CONSTANTS, MEMPOOL_MODE)
            .map_err(|error| format!("consensus/signature validation failed: {error:?}"))?;
    let expected_removals = u128::from(latest.funding_amount) + u128::from(fee.coin.amount);
    let expected_additions = expected_removals
        .checked_sub(u128::from(fee.fee_mojo))
        .ok_or("fee exceeds removals")?;
    if conditions.removal_amount != expected_removals
        || conditions.addition_amount != expected_additions
        || conditions.reserve_fee != fee.fee_mojo
    {
        return Err("fee or amount conservation did not match the requested bundle".into());
    }
    if conditions.before_height_absolute != Some(to_u32(challenge_deadline_height, "deadline")?) {
        return Err("consensus deadline differs from the monitored deadline".into());
    }
    let next_coin = Coin::new(
        snapshot.closing_coin.coin_id(),
        material.expected_next_closing_puzzle_hash.into(),
        latest.funding_amount,
    );
    let fee_coin_id = fee.coin.coin_id();
    let commitment = spend_bundle_commitment(&spend_bundle)?;
    Ok(OfflineChallengeBundle {
        spend_bundle,
        construction_snapshot: snapshot.clone(),
        commitment,
        report: OfflineBundleReport {
            schema: "xhub-v3.6-offline-challenge-bundle-1",
            closing_coin_id: hex::encode(snapshot.closing_coin.coin_id()),
            next_closing_coin_id: hex::encode(next_coin.coin_id()),
            fee_coin_id: hex::encode(fee_coin_id),
            fee_mojo: fee.fee_mojo,
            removal_amount_mojo: conditions.removal_amount,
            addition_amount_mojo: conditions.addition_amount,
            cost: conditions.cost,
            consensus_conditions_verified: true,
            aggregate_signature_verified: true,
            spend_bundle_created: true,
            broadcast_enabled: false,
            broadcast_ready: false,
            chain_broadcast: false,
        },
    })
}

fn spend_bundle_commitment(bundle: &SpendBundle) -> Result<Bytes32, String> {
    let mut material = Vec::new();
    let spend_count = u32::try_from(bundle.coin_spends.len())
        .map_err(|_| "SpendBundle CoinSpend count exceeds u32")?;
    material.extend_from_slice(&spend_count.to_be_bytes());
    for spend in &bundle.coin_spends {
        material.extend_from_slice(spend.coin.parent_coin_info.as_ref());
        material.extend_from_slice(spend.coin.puzzle_hash.as_ref());
        material.extend_from_slice(&spend.coin.amount.to_be_bytes());
        put_commitment_bytes(&mut material, spend.puzzle_reveal.as_slice())?;
        put_commitment_bytes(&mut material, spend.solution.as_slice())?;
    }
    material.extend_from_slice(&bundle.aggregated_signature.to_bytes());
    Ok(sha256_parts(&[SPEND_BUNDLE_COMMITMENT_DOMAIN, &material]))
}

fn put_commitment_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    let length = u64::try_from(bytes.len()).map_err(|_| "commitment field exceeds u64")?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn validate_closing_coin(
    snapshot: &ChainSnapshot,
    material: &ChallengeSpendMaterial,
    amount: u64,
) -> Result<(), String> {
    if snapshot.closing_coin.coin_id().to_bytes() != snapshot.closing_coin_id
        || snapshot.closing_coin.puzzle_hash != material.expected_closing_puzzle_hash.into()
        || snapshot.closing_coin.amount != amount
        || snapshot.closing_birth_height == 0
    {
        return Err(
            "Closing Coin identity, puzzle hash, amount, or birth height is invalid".into(),
        );
    }
    let reveal_hash = program_hash(&material.puzzle_reveal)?;
    if reveal_hash != material.expected_closing_puzzle_hash {
        return Err("Closing puzzle reveal does not match the observed puzzle hash".into());
    }
    Ok(())
}

fn fee_spend(fee: &TestFeeSponsor) -> Result<CoinSpend, String> {
    let public_key = fee.secret_key.public_key();
    let args = P2DelegatedConditionsArgs::new(public_key);
    let change = fee
        .coin
        .amount
        .checked_sub(fee.fee_mojo)
        .ok_or("fee exceeds fee Coin")?;
    let mut allocator = Allocator::new();
    let fee_module = clvmr::serde::node_from_bytes(
        &mut allocator,
        P2DelegatedConditionsArgs::mod_reveal().as_ref(),
    )
    .map_err(|error| format!("fee module decoding failed: {error}"))?;
    let puzzle_node = CurriedProgram {
        program: fee_module,
        args: &args,
    }
    .to_clvm(&mut allocator)
    .map_err(|error| format!("fee puzzle curry failed: {error:?}"))?;
    let solution_node = fee_solution(&mut allocator, fee.change_puzzle_hash, change, fee.fee_mojo)?;
    let puzzle = Program::from(
        node_to_bytes(&allocator, puzzle_node)
            .map_err(|error| format!("fee puzzle serialization failed: {error}"))?,
    );
    if program_hash(&puzzle)? != fee.coin.puzzle_hash.to_bytes() {
        return Err("fee Coin puzzle hash does not match the test fee key".into());
    }
    let solution = Program::from(
        node_to_bytes(&allocator, solution_node)
            .map_err(|error| format!("fee solution serialization failed: {error}"))?,
    );
    let spend = CoinSpend::new(fee.coin, puzzle, solution);
    Ok(spend)
}

pub fn test_fee_coin(
    parent_coin_id: Bytes32,
    amount: u64,
    secret_key: SecretKey,
    change_puzzle_hash: Bytes32,
    fee_mojo: u64,
) -> Result<TestFeeSponsor, String> {
    let args = P2DelegatedConditionsArgs::new(secret_key.public_key());
    let puzzle_hash = args.curry_tree_hash().to_bytes();
    Ok(TestFeeSponsor {
        coin: Coin::new(parent_coin_id.into(), puzzle_hash.into(), amount),
        secret_key,
        change_puzzle_hash,
        fee_mojo,
    })
}

fn validate_unique_removals(bundle: &SpendBundle) -> Result<(), String> {
    let mut ids = HashSet::with_capacity(bundle.coin_spends.len());
    for spend in &bundle.coin_spends {
        if !ids.insert(spend.coin.coin_id()) {
            return Err("SpendBundle contains duplicate removals".into());
        }
    }
    Ok(())
}

fn program_hash(program: &Program) -> Result<Bytes32, String> {
    let mut allocator = Allocator::new();
    let node = clvmr::serde::node_from_bytes(&mut allocator, program.as_slice())
        .map_err(|error| format!("invalid serialized Program: {error}"))?;
    Ok(tree_hash(&allocator, node).to_bytes())
}

fn fee_solution(
    allocator: &mut Allocator,
    change_puzzle_hash: Bytes32,
    change: u64,
    fee: u64,
) -> Result<NodePtr, String> {
    let create_opcode = atom_u8(allocator, 51)?;
    let create_hash = atom(allocator, &change_puzzle_hash)?;
    let create_amount = number(allocator, change)?;
    let create = list(allocator, &[create_opcode, create_hash, create_amount])?;
    let reserve_opcode = atom_u8(allocator, 52)?;
    let reserve_amount = number(allocator, fee)?;
    let reserve = list(allocator, &[reserve_opcode, reserve_amount])?;
    let conditions = list(allocator, &[create, reserve])?;
    list(allocator, &[conditions])
}

fn list(allocator: &mut Allocator, items: &[NodePtr]) -> Result<NodePtr, String> {
    items.iter().rev().try_fold(allocator.nil(), |tail, item| {
        allocator
            .new_pair(*item, tail)
            .map_err(|error| format!("CLVM list allocation failed: {error}"))
    })
}

fn atom(allocator: &mut Allocator, bytes: &[u8]) -> Result<NodePtr, String> {
    allocator
        .new_atom(bytes)
        .map_err(|error| format!("CLVM atom allocation failed: {error}"))
}

fn atom_u8(allocator: &mut Allocator, value: u8) -> Result<NodePtr, String> {
    atom(allocator, &[value])
}

fn number(allocator: &mut Allocator, value: u64) -> Result<NodePtr, String> {
    allocator
        .new_number(value.into())
        .map_err(|error| format!("CLVM number allocation failed: {error}"))
}

fn to_u32(value: u64, field: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field} exceeds Chia height range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chia_bls::{Signature, sign};

    #[test]
    fn duplicate_removals_are_rejected_before_consensus_validation() {
        let coin = Coin::new([1; 32].into(), [2; 32].into(), 10);
        let spend = CoinSpend::new(coin, Program::default(), Program::default());
        let bundle = SpendBundle {
            coin_spends: vec![spend.clone(), spend],
            aggregated_signature: Signature::default(),
        };
        assert_eq!(
            validate_unique_removals(&bundle).unwrap_err(),
            "SpendBundle contains duplicate removals"
        );
    }

    #[test]
    fn commitment_covers_coin_order_programs_and_aggregate_signature() {
        let first = CoinSpend::new(
            Coin::new([1; 32].into(), [2; 32].into(), 10),
            Program::from(vec![0x80]),
            Program::from(vec![0x81, 1]),
        );
        let second = CoinSpend::new(
            Coin::new([3; 32].into(), [4; 32].into(), 20),
            Program::from(vec![0x81, 2]),
            Program::from(vec![0x80]),
        );
        let base = SpendBundle {
            coin_spends: vec![first.clone(), second.clone()],
            aggregated_signature: Signature::default(),
        };
        let expected = spend_bundle_commitment(&base).expect("commitment");
        assert_eq!(expected, spend_bundle_commitment(&base).expect("stable"));

        let reordered = SpendBundle {
            coin_spends: vec![second, first.clone()],
            aggregated_signature: Signature::default(),
        };
        assert_ne!(
            expected,
            spend_bundle_commitment(&reordered).expect("ordered commitment")
        );
        let changed_solution = SpendBundle {
            coin_spends: vec![CoinSpend::new(
                first.coin,
                first.puzzle_reveal,
                Program::from(vec![0x81, 9]),
            )],
            aggregated_signature: Signature::default(),
        };
        assert_ne!(
            expected,
            spend_bundle_commitment(&changed_solution).expect("solution commitment")
        );
        let changed_signature = SpendBundle {
            coin_spends: base.coin_spends,
            aggregated_signature: sign(&SecretKey::from_seed(&[7; 32]), b"commitment-test"),
        };
        assert_ne!(
            expected,
            spend_bundle_commitment(&changed_signature).expect("signature commitment")
        );
    }
}
