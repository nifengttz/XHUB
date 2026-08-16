use chia_bls::PublicKey;
use chia_bls::Signature;
use chia_protocol::{Bytes, Bytes32, Coin, Program};
use chia_sdk_types::run_puzzle_with_cost;
use clvm_traits::ToClvm;
use clvm_utils::CurriedProgram;
use clvmr::{
    Allocator, NodePtr, SExp,
    serde::{node_from_bytes, node_to_bytes},
};
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{
    LedgerEntry, PROTOCOL_VERSION, RecoveryPackage, StateZero, closing_state_hash,
    merchant_payment_puzzle_hash, one_arg_puzzle_hash,
};

use crate::{
    FUNDING_HEX, INITIAL_CLOSING_HEX, MERCHANT_PAYMENT_HEX, SUBSEQUENT_CLOSING_HEX, module_bytes,
    module_hashes,
};

const MAX_COST: u64 = 11_000_000_000;
const AGG_SIG_UNSAFE: u64 = 49;
const CREATE_COIN: u64 = 51;
const ASSERT_MY_COIN_ID: u64 = 70;
const ASSERT_MY_AMOUNT: u64 = 73;
const ASSERT_MY_BIRTH_HEIGHT: u64 = 75;
const ASSERT_HEIGHT_ABSOLUTE: u64 = 83;
const ASSERT_HEIGHT_RELATIVE: u64 = 82;
const ASSERT_BEFORE_HEIGHT_ABSOLUTE: u64 = 87;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosingSimulation {
    pub protocol_version: &'static str,
    pub funding_coin_id: String,
    pub state_sequence: u64,
    pub checkpoint_hash: String,
    pub funding_amount_mojo: u64,
    pub reserved_total_mojo: u64,
    pub user_remainder_mojo: u64,
    pub entry_count: u64,
    pub hypothetical_start_close_height: u64,
    pub challenge_deadline_height: u64,
    pub funding: FundingSimulation,
    pub initial_finalize: FinalizeSimulation,
    pub merchant_forwards: Vec<MerchantForwardSimulation>,
    pub recovery_package_verified: bool,
    pub all_clvm_conditions_verified: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FundingSimulation {
    pub cost: u64,
    pub assert_height_relative: u64,
    pub assert_my_coin_id: String,
    pub assert_my_amount_mojo: u64,
    pub agg_sig_condition_count: u64,
    pub initial_closing_puzzle_hash: String,
    pub initial_closing_coin_id: String,
    pub initial_closing_amount_mojo: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FinalizeSimulation {
    pub cost: u64,
    pub assert_my_birth_height: u64,
    pub assert_height_absolute: u64,
    pub outputs: Vec<ClosingOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosingOutput {
    pub kind: &'static str,
    pub entry_index: Option<u64>,
    pub puzzle_hash: String,
    pub amount_mojo: u64,
    pub coin_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MerchantForwardSimulation {
    pub entry_index: u64,
    pub payment_coin_id: String,
    pub payment_puzzle_hash: String,
    pub amount_mojo: u64,
    pub merchant_puzzle_hash: String,
    pub cost: u64,
    pub assert_my_amount_mojo: u64,
    pub forwarded_amount_mojo: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClosingCoinKind {
    Initial,
    Subsequent,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChallengeSimulation {
    pub protocol_version: String,
    pub funding_coin_id: String,
    pub closing_coin_kind: ClosingCoinKind,
    pub current_state_sequence: u64,
    pub current_checkpoint_hash: String,
    pub latest_state_sequence: u64,
    pub latest_checkpoint_hash: String,
    pub initial_birth_height: u64,
    pub challenge_deadline_height: u64,
    pub current_closing_puzzle_hash: String,
    pub next_closing_puzzle_hash: String,
    pub closing_amount_mojo: u64,
    pub cost: u64,
    pub assert_my_birth_height: Option<u64>,
    pub assert_before_height_absolute: u64,
    pub agg_sig_condition_count: u64,
    pub recovery_packages_verified: bool,
    pub all_clvm_conditions_verified: bool,
    pub spend_bundle_created: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

#[derive(Debug, Clone)]
pub struct ChallengeSpendMaterial {
    pub simulation: ChallengeSimulation,
    pub puzzle_reveal: Program,
    pub solution: Program,
    pub expected_closing_puzzle_hash: [u8; 32],
    pub expected_next_closing_puzzle_hash: [u8; 32],
    pub protocol_signatures: Vec<Signature>,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct ClvmEntry {
    entry_index: Bytes,
    merchant_puzzle_hash: Bytes32,
    merchant_receipt_public_key: PublicKey,
    amount: Bytes,
    reservation_nonce: Bytes32,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(curry)]
struct FundingArgs {
    network_id: Bytes32,
    acceptance_blocks: Bytes,
    freeze_blocks: Bytes,
    close_delay_blocks: Bytes,
    challenge_blocks: Bytes,
    user_public_key: PublicKey,
    hub_public_key: PublicKey,
    state_rules_hash: Bytes32,
    funding_amount: Bytes,
    user_remainder_puzzle_hash: Bytes32,
    max_ledger_entries: Bytes,
    initial_closing_mod_hash: Bytes32,
    subsequent_closing_mod_hash: Bytes32,
    payment_mod_hash: Bytes32,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct FundingSolution {
    funding_coin_id: Bytes32,
    state_sequence: Bytes,
    previous_checkpoint_hash: Bytes32,
    manifest_root: Bytes32,
    entry_count: Bytes,
    reserved_total: Bytes,
    user_remainder: Bytes,
    entries: Vec<ClvmEntry>,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(curry)]
struct ClosingArgs {
    current_commitment: Bytes32,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct InitialClosingSolution {
    mode: u8,
    initial_birth_height: Bytes,
    challenge_deadline_height: Bytes,
    network_id: Bytes32,
    acceptance_blocks: Bytes,
    freeze_blocks: Bytes,
    close_delay_blocks: Bytes,
    challenge_blocks: Bytes,
    user_public_key: PublicKey,
    hub_public_key: PublicKey,
    state_rules_hash: Bytes32,
    funding_amount: Bytes,
    user_remainder_puzzle_hash: Bytes32,
    max_ledger_entries: Bytes,
    initial_closing_mod_hash: Bytes32,
    subsequent_closing_mod_hash: Bytes32,
    payment_mod_hash: Bytes32,
    funding_coin_id: Bytes32,
    current_sequence: Bytes,
    current_previous_checkpoint_hash: Bytes32,
    current_manifest_root: Bytes32,
    current_entry_count: Bytes,
    current_reserved_total: Bytes,
    current_user_remainder: Bytes,
    current_entries: Vec<ClvmEntry>,
    new_sequence: Bytes,
    new_previous_checkpoint_hash: Bytes32,
    new_manifest_root: Bytes32,
    new_entry_count: Bytes,
    new_reserved_total: Bytes,
    new_user_remainder: Bytes,
    new_entries: Vec<ClvmEntry>,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct SubsequentClosingSolution {
    mode: u8,
    challenge_deadline_height: Bytes,
    network_id: Bytes32,
    acceptance_blocks: Bytes,
    freeze_blocks: Bytes,
    close_delay_blocks: Bytes,
    challenge_blocks: Bytes,
    user_public_key: PublicKey,
    hub_public_key: PublicKey,
    state_rules_hash: Bytes32,
    funding_amount: Bytes,
    user_remainder_puzzle_hash: Bytes32,
    max_ledger_entries: Bytes,
    initial_closing_mod_hash: Bytes32,
    subsequent_closing_mod_hash: Bytes32,
    payment_mod_hash: Bytes32,
    funding_coin_id: Bytes32,
    current_sequence: Bytes,
    current_previous_checkpoint_hash: Bytes32,
    current_manifest_root: Bytes32,
    current_entry_count: Bytes,
    current_reserved_total: Bytes,
    current_user_remainder: Bytes,
    current_entries: Vec<ClvmEntry>,
    new_sequence: Bytes,
    new_previous_checkpoint_hash: Bytes32,
    new_manifest_root: Bytes32,
    new_entry_count: Bytes,
    new_reserved_total: Bytes,
    new_user_remainder: Bytes,
    new_entries: Vec<ClvmEntry>,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(curry)]
struct PaymentArgs {
    protocol_version: Bytes,
    network_id: Bytes32,
    funding_coin_id: Bytes32,
    channel_terms_hash: Bytes32,
    entry_index: Bytes,
    reservation_nonce: Bytes32,
    merchant_puzzle_hash: Bytes32,
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(list)]
struct PaymentSolution {
    payment_coin_amount: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Condition {
    opcode: u64,
    args: Vec<Vec<u8>>,
}

pub fn simulate_recovery_closing(
    package: &RecoveryPackage,
    hypothetical_start_close_height: u64,
) -> Result<ClosingSimulation, String> {
    package.validate().map_err(|error| error.to_string())?;
    if hypothetical_start_close_height == 0 {
        return Err("hypothetical start-close height must be positive".into());
    }
    let terms = &package.channel_terms;
    let checkpoint = &package.official_state.checkpoint;
    let checkpoint_hash = checkpoint.hash(terms).map_err(|error| error.to_string())?;
    let terms_hash = terms.hash().map_err(|error| error.to_string())?;
    let hashes = module_hashes();
    let entries = clvm_entries(&package.entries)?;
    let funding_args = FundingArgs {
        network_id: terms.network_id.into(),
        acceptance_blocks: fixed(terms.acceptance_blocks),
        freeze_blocks: fixed(terms.freeze_blocks),
        close_delay_blocks: fixed(terms.close_delay_blocks),
        challenge_blocks: fixed(terms.challenge_blocks),
        user_public_key: public_key(&terms.user_public_key, "user")?,
        hub_public_key: public_key(&terms.hub_state_public_key_a, "Hub")?,
        state_rules_hash: terms.state_rules_hash.into(),
        funding_amount: fixed(terms.funding_amount),
        user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
        max_ledger_entries: fixed(terms.max_ledger_entries),
        initial_closing_mod_hash: hashes.initial_closing.into(),
        subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
        payment_mod_hash: hashes.merchant_payment.into(),
    };
    let funding_solution = FundingSolution {
        funding_coin_id: package.funding_coin_id.into(),
        state_sequence: fixed(checkpoint.state_sequence),
        previous_checkpoint_hash: checkpoint.previous_checkpoint_hash.into(),
        manifest_root: checkpoint.manifest_root.into(),
        entry_count: fixed(checkpoint.entry_count),
        reserved_total: fixed(checkpoint.reserved_total),
        user_remainder: fixed(checkpoint.user_remainder),
        entries: entries.clone(),
    };
    let (funding_cost, funding_conditions) =
        run_curried(FUNDING_HEX, &funding_args, &funding_solution)?;
    require_u64(
        &funding_conditions,
        ASSERT_HEIGHT_RELATIVE,
        terms.close_delay_blocks,
    )?;
    require_bytes(
        &funding_conditions,
        ASSERT_MY_COIN_ID,
        &package.funding_coin_id,
    )?;
    require_u64(&funding_conditions, ASSERT_MY_AMOUNT, terms.funding_amount)?;
    if conditions(&funding_conditions, AGG_SIG_UNSAFE).len() != package.entries.len() + 1 {
        return Err("Funding CLVM emitted an unexpected signature condition count".into());
    }
    let initial_commitment = closing_state_hash(
        &terms.network_id,
        &package.funding_coin_id,
        &terms_hash,
        &[0; 8],
        &checkpoint_hash,
    );
    let initial_puzzle_hash = one_arg_puzzle_hash(hashes.initial_closing, &initial_commitment);
    require_single_create(
        &funding_conditions,
        &initial_puzzle_hash,
        terms.funding_amount,
    )?;
    let initial_coin = Coin::new(
        package.funding_coin_id.into(),
        initial_puzzle_hash.into(),
        terms.funding_amount,
    );
    let initial_coin_id = initial_coin.coin_id().to_bytes();

    let deadline = hypothetical_start_close_height
        .checked_add(terms.challenge_blocks)
        .ok_or("challenge deadline overflow")?;
    let closing_args = ClosingArgs {
        current_commitment: initial_commitment.into(),
    };
    let finalize_solution = InitialClosingSolution {
        mode: 2,
        initial_birth_height: fixed(hypothetical_start_close_height),
        challenge_deadline_height: fixed(deadline),
        network_id: terms.network_id.into(),
        acceptance_blocks: fixed(terms.acceptance_blocks),
        freeze_blocks: fixed(terms.freeze_blocks),
        close_delay_blocks: fixed(terms.close_delay_blocks),
        challenge_blocks: fixed(terms.challenge_blocks),
        user_public_key: public_key(&terms.user_public_key, "user")?,
        hub_public_key: public_key(&terms.hub_state_public_key_a, "Hub")?,
        state_rules_hash: terms.state_rules_hash.into(),
        funding_amount: fixed(terms.funding_amount),
        user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
        max_ledger_entries: fixed(terms.max_ledger_entries),
        initial_closing_mod_hash: hashes.initial_closing.into(),
        subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
        payment_mod_hash: hashes.merchant_payment.into(),
        funding_coin_id: package.funding_coin_id.into(),
        current_sequence: fixed(checkpoint.state_sequence),
        current_previous_checkpoint_hash: checkpoint.previous_checkpoint_hash.into(),
        current_manifest_root: checkpoint.manifest_root.into(),
        current_entry_count: fixed(checkpoint.entry_count),
        current_reserved_total: fixed(checkpoint.reserved_total),
        current_user_remainder: fixed(checkpoint.user_remainder),
        current_entries: entries.clone(),
        new_sequence: fixed(checkpoint.state_sequence),
        new_previous_checkpoint_hash: checkpoint.previous_checkpoint_hash.into(),
        new_manifest_root: checkpoint.manifest_root.into(),
        new_entry_count: fixed(checkpoint.entry_count),
        new_reserved_total: fixed(checkpoint.reserved_total),
        new_user_remainder: fixed(checkpoint.user_remainder),
        new_entries: entries,
    };
    let (finalize_cost, finalize_conditions) =
        run_curried(INITIAL_CLOSING_HEX, &closing_args, &finalize_solution)?;
    require_u64(
        &finalize_conditions,
        ASSERT_MY_BIRTH_HEIGHT,
        hypothetical_start_close_height,
    )?;
    require_u64(&finalize_conditions, ASSERT_HEIGHT_ABSOLUTE, deadline)?;
    let creates = conditions(&finalize_conditions, CREATE_COIN);
    if creates.len() != package.entries.len() + 1 {
        return Err("Initial Closing FINALIZE emitted an unexpected output count".into());
    }

    let mut outputs = Vec::with_capacity(creates.len());
    let mut merchant_forwards = Vec::with_capacity(package.entries.len());
    for (index, entry) in package.entries.iter().enumerate() {
        let payment_hash = merchant_payment_puzzle_hash(
            hashes.merchant_payment,
            &terms.network_id,
            &package.funding_coin_id,
            &terms_hash,
            index as u64,
            &entry.reservation_nonce,
            &entry.merchant_puzzle_hash,
        );
        require_create(creates[index], &payment_hash, entry.amount)?;
        let payment_coin = Coin::new(initial_coin_id.into(), payment_hash.into(), entry.amount);
        let payment_coin_id = payment_coin.coin_id().to_bytes();
        outputs.push(ClosingOutput {
            kind: "MERCHANT_PAYMENT",
            entry_index: Some(index as u64),
            puzzle_hash: hex::encode(payment_hash),
            amount_mojo: entry.amount,
            coin_id: hex::encode(payment_coin_id),
        });

        let payment_args = PaymentArgs {
            protocol_version: PROTOCOL_VERSION.to_be_bytes().to_vec().into(),
            network_id: terms.network_id.into(),
            funding_coin_id: package.funding_coin_id.into(),
            channel_terms_hash: terms_hash.into(),
            entry_index: fixed(index as u64),
            reservation_nonce: entry.reservation_nonce.into(),
            merchant_puzzle_hash: entry.merchant_puzzle_hash.into(),
        };
        let payment_solution = PaymentSolution {
            payment_coin_amount: fixed(entry.amount),
        };
        let (cost, payment_conditions) =
            run_curried(MERCHANT_PAYMENT_HEX, &payment_args, &payment_solution)?;
        require_u64(&payment_conditions, ASSERT_MY_AMOUNT, entry.amount)?;
        require_single_create(
            &payment_conditions,
            &entry.merchant_puzzle_hash,
            entry.amount,
        )?;
        merchant_forwards.push(MerchantForwardSimulation {
            entry_index: index as u64,
            payment_coin_id: hex::encode(payment_coin_id),
            payment_puzzle_hash: hex::encode(payment_hash),
            amount_mojo: entry.amount,
            merchant_puzzle_hash: hex::encode(entry.merchant_puzzle_hash),
            cost,
            assert_my_amount_mojo: entry.amount,
            forwarded_amount_mojo: entry.amount,
        });
    }
    let remainder_create = creates
        .last()
        .ok_or("Initial Closing FINALIZE omitted the user remainder")?;
    require_create(
        remainder_create,
        &terms.user_remainder_puzzle_hash,
        checkpoint.user_remainder,
    )?;
    let remainder_coin = Coin::new(
        initial_coin_id.into(),
        terms.user_remainder_puzzle_hash.into(),
        checkpoint.user_remainder,
    );
    outputs.push(ClosingOutput {
        kind: "USER_REMAINDER",
        entry_index: None,
        puzzle_hash: hex::encode(terms.user_remainder_puzzle_hash),
        amount_mojo: checkpoint.user_remainder,
        coin_id: hex::encode(remainder_coin.coin_id()),
    });

    Ok(ClosingSimulation {
        protocol_version: "0x0360",
        funding_coin_id: hex::encode(package.funding_coin_id),
        state_sequence: checkpoint.state_sequence,
        checkpoint_hash: hex::encode(checkpoint_hash),
        funding_amount_mojo: terms.funding_amount,
        reserved_total_mojo: checkpoint.reserved_total,
        user_remainder_mojo: checkpoint.user_remainder,
        entry_count: checkpoint.entry_count,
        hypothetical_start_close_height,
        challenge_deadline_height: deadline,
        funding: FundingSimulation {
            cost: funding_cost,
            assert_height_relative: terms.close_delay_blocks,
            assert_my_coin_id: hex::encode(package.funding_coin_id),
            assert_my_amount_mojo: terms.funding_amount,
            agg_sig_condition_count: (package.entries.len() + 1) as u64,
            initial_closing_puzzle_hash: hex::encode(initial_puzzle_hash),
            initial_closing_coin_id: hex::encode(initial_coin_id),
            initial_closing_amount_mojo: terms.funding_amount,
        },
        initial_finalize: FinalizeSimulation {
            cost: finalize_cost,
            assert_my_birth_height: hypothetical_start_close_height,
            assert_height_absolute: deadline,
            outputs,
        },
        merchant_forwards,
        recovery_package_verified: true,
        all_clvm_conditions_verified: true,
        broadcast_ready: false,
        chain_broadcast: false,
    })
}

pub fn simulate_challenge(
    current: &RecoveryPackage,
    latest: &RecoveryPackage,
    closing_coin_kind: ClosingCoinKind,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<ChallengeSimulation, String> {
    Ok(challenge_spend_material(
        current,
        latest,
        closing_coin_kind,
        initial_birth_height,
        challenge_deadline_height,
    )?
    .simulation)
}

pub fn challenge_spend_material(
    current: &RecoveryPackage,
    latest: &RecoveryPackage,
    closing_coin_kind: ClosingCoinKind,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<ChallengeSpendMaterial, String> {
    current.validate().map_err(|error| error.to_string())?;
    latest.validate().map_err(|error| error.to_string())?;
    if current.funding_coin_id != latest.funding_coin_id
        || current.channel_terms != latest.channel_terms
        || current.funding_amount != latest.funding_amount
    {
        return Err("current and latest RecoveryPackages describe different channels".into());
    }
    let current_checkpoint = &current.official_state.checkpoint;
    let latest_checkpoint = &latest.official_state.checkpoint;
    if latest_checkpoint.state_sequence <= current_checkpoint.state_sequence {
        return Err("CHALLENGE requires a strictly higher latest state sequence".into());
    }
    let terms = &latest.channel_terms;
    if initial_birth_height == 0 {
        return Err("initial Closing Coin birth height must be positive".into());
    }
    let expected_deadline = initial_birth_height
        .checked_add(terms.challenge_blocks)
        .ok_or("challenge deadline overflow")?;
    if challenge_deadline_height != expected_deadline {
        return Err(
            "challenge deadline does not equal initial birth height plus challenge_blocks".into(),
        );
    }

    let current_checkpoint_hash = current_checkpoint
        .hash(terms)
        .map_err(|error| error.to_string())?;
    let latest_checkpoint_hash = latest_checkpoint
        .hash(terms)
        .map_err(|error| error.to_string())?;
    let terms_hash = terms.hash().map_err(|error| error.to_string())?;
    let hashes = module_hashes();
    let deadline_bytes = challenge_deadline_height.to_be_bytes();
    let current_deadline = match closing_coin_kind {
        ClosingCoinKind::Initial => [0; 8],
        ClosingCoinKind::Subsequent => deadline_bytes,
    };
    let current_commitment = closing_state_hash(
        &terms.network_id,
        &latest.funding_coin_id,
        &terms_hash,
        &current_deadline,
        &current_checkpoint_hash,
    );
    let current_module_hash = match closing_coin_kind {
        ClosingCoinKind::Initial => hashes.initial_closing,
        ClosingCoinKind::Subsequent => hashes.subsequent_closing,
    };
    let current_puzzle_hash = one_arg_puzzle_hash(current_module_hash, &current_commitment);
    let next_commitment = closing_state_hash(
        &terms.network_id,
        &latest.funding_coin_id,
        &terms_hash,
        &deadline_bytes,
        &latest_checkpoint_hash,
    );
    let next_puzzle_hash = one_arg_puzzle_hash(hashes.subsequent_closing, &next_commitment);
    let args = ClosingArgs {
        current_commitment: current_commitment.into(),
    };
    let current_entries = clvm_entries(&current.entries)?;
    let latest_entries = clvm_entries(&latest.entries)?;
    let (cost, challenge_conditions, puzzle_reveal, solution) = match closing_coin_kind {
        ClosingCoinKind::Initial => run_curried_material(
            INITIAL_CLOSING_HEX,
            &args,
            &InitialClosingSolution {
                mode: 1,
                initial_birth_height: fixed(initial_birth_height),
                challenge_deadline_height: fixed(challenge_deadline_height),
                network_id: terms.network_id.into(),
                acceptance_blocks: fixed(terms.acceptance_blocks),
                freeze_blocks: fixed(terms.freeze_blocks),
                close_delay_blocks: fixed(terms.close_delay_blocks),
                challenge_blocks: fixed(terms.challenge_blocks),
                user_public_key: public_key(&terms.user_public_key, "user")?,
                hub_public_key: public_key(&terms.hub_state_public_key_a, "Hub")?,
                state_rules_hash: terms.state_rules_hash.into(),
                funding_amount: fixed(terms.funding_amount),
                user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
                max_ledger_entries: fixed(terms.max_ledger_entries),
                initial_closing_mod_hash: hashes.initial_closing.into(),
                subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
                payment_mod_hash: hashes.merchant_payment.into(),
                funding_coin_id: latest.funding_coin_id.into(),
                current_sequence: fixed(current_checkpoint.state_sequence),
                current_previous_checkpoint_hash: current_checkpoint
                    .previous_checkpoint_hash
                    .into(),
                current_manifest_root: current_checkpoint.manifest_root.into(),
                current_entry_count: fixed(current_checkpoint.entry_count),
                current_reserved_total: fixed(current_checkpoint.reserved_total),
                current_user_remainder: fixed(current_checkpoint.user_remainder),
                current_entries,
                new_sequence: fixed(latest_checkpoint.state_sequence),
                new_previous_checkpoint_hash: latest_checkpoint.previous_checkpoint_hash.into(),
                new_manifest_root: latest_checkpoint.manifest_root.into(),
                new_entry_count: fixed(latest_checkpoint.entry_count),
                new_reserved_total: fixed(latest_checkpoint.reserved_total),
                new_user_remainder: fixed(latest_checkpoint.user_remainder),
                new_entries: latest_entries,
            },
        )?,
        ClosingCoinKind::Subsequent => run_curried_material(
            SUBSEQUENT_CLOSING_HEX,
            &args,
            &SubsequentClosingSolution {
                mode: 1,
                challenge_deadline_height: fixed(challenge_deadline_height),
                network_id: terms.network_id.into(),
                acceptance_blocks: fixed(terms.acceptance_blocks),
                freeze_blocks: fixed(terms.freeze_blocks),
                close_delay_blocks: fixed(terms.close_delay_blocks),
                challenge_blocks: fixed(terms.challenge_blocks),
                user_public_key: public_key(&terms.user_public_key, "user")?,
                hub_public_key: public_key(&terms.hub_state_public_key_a, "Hub")?,
                state_rules_hash: terms.state_rules_hash.into(),
                funding_amount: fixed(terms.funding_amount),
                user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
                max_ledger_entries: fixed(terms.max_ledger_entries),
                initial_closing_mod_hash: hashes.initial_closing.into(),
                subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
                payment_mod_hash: hashes.merchant_payment.into(),
                funding_coin_id: latest.funding_coin_id.into(),
                current_sequence: fixed(current_checkpoint.state_sequence),
                current_previous_checkpoint_hash: current_checkpoint
                    .previous_checkpoint_hash
                    .into(),
                current_manifest_root: current_checkpoint.manifest_root.into(),
                current_entry_count: fixed(current_checkpoint.entry_count),
                current_reserved_total: fixed(current_checkpoint.reserved_total),
                current_user_remainder: fixed(current_checkpoint.user_remainder),
                current_entries,
                new_sequence: fixed(latest_checkpoint.state_sequence),
                new_previous_checkpoint_hash: latest_checkpoint.previous_checkpoint_hash.into(),
                new_manifest_root: latest_checkpoint.manifest_root.into(),
                new_entry_count: fixed(latest_checkpoint.entry_count),
                new_reserved_total: fixed(latest_checkpoint.reserved_total),
                new_user_remainder: fixed(latest_checkpoint.user_remainder),
                new_entries: latest_entries,
            },
        )?,
    };
    require_u64(
        &challenge_conditions,
        ASSERT_BEFORE_HEIGHT_ABSOLUTE,
        challenge_deadline_height,
    )?;
    require_u64(
        &challenge_conditions,
        ASSERT_MY_AMOUNT,
        terms.funding_amount,
    )?;
    if closing_coin_kind == ClosingCoinKind::Initial {
        require_u64(
            &challenge_conditions,
            ASSERT_MY_BIRTH_HEIGHT,
            initial_birth_height,
        )?;
    } else if !conditions(&challenge_conditions, ASSERT_MY_BIRTH_HEIGHT).is_empty() {
        return Err("Subsequent Closing CHALLENGE unexpectedly asserted a birth height".into());
    }
    require_single_create(
        &challenge_conditions,
        &next_puzzle_hash,
        terms.funding_amount,
    )?;
    let expected_signatures = current.entries.len() + latest.entries.len() + 2;
    if conditions(&challenge_conditions, AGG_SIG_UNSAFE).len() != expected_signatures {
        return Err("Closing CHALLENGE emitted an unexpected signature condition count".into());
    }

    let simulation = ChallengeSimulation {
        protocol_version: "0x0360".into(),
        funding_coin_id: hex::encode(latest.funding_coin_id),
        closing_coin_kind,
        current_state_sequence: current_checkpoint.state_sequence,
        current_checkpoint_hash: hex::encode(current_checkpoint_hash),
        latest_state_sequence: latest_checkpoint.state_sequence,
        latest_checkpoint_hash: hex::encode(latest_checkpoint_hash),
        initial_birth_height,
        challenge_deadline_height,
        current_closing_puzzle_hash: hex::encode(current_puzzle_hash),
        next_closing_puzzle_hash: hex::encode(next_puzzle_hash),
        closing_amount_mojo: terms.funding_amount,
        cost,
        assert_my_birth_height: (closing_coin_kind == ClosingCoinKind::Initial)
            .then_some(initial_birth_height),
        assert_before_height_absolute: challenge_deadline_height,
        agg_sig_condition_count: expected_signatures as u64,
        recovery_packages_verified: true,
        all_clvm_conditions_verified: true,
        spend_bundle_created: false,
        broadcast_ready: false,
        chain_broadcast: false,
    };
    let protocol_signatures = package_signatures(current)?
        .into_iter()
        .chain(package_signatures(latest)?)
        .collect();
    Ok(ChallengeSpendMaterial {
        simulation,
        puzzle_reveal,
        solution,
        expected_closing_puzzle_hash: current_puzzle_hash,
        expected_next_closing_puzzle_hash: next_puzzle_hash,
        protocol_signatures,
    })
}

pub fn simulate_state_zero_challenge(
    latest: &RecoveryPackage,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<ChallengeSimulation, String> {
    Ok(state_zero_challenge_spend_material(
        latest,
        initial_birth_height,
        challenge_deadline_height,
    )?
    .simulation)
}

pub fn state_zero_challenge_spend_material(
    latest: &RecoveryPackage,
    initial_birth_height: u64,
    challenge_deadline_height: u64,
) -> Result<ChallengeSpendMaterial, String> {
    latest.validate().map_err(|error| error.to_string())?;
    let terms = &latest.channel_terms;
    let latest_checkpoint = &latest.official_state.checkpoint;
    if latest_checkpoint.state_sequence == 0 || initial_birth_height == 0 {
        return Err(
            "State 0 CHALLENGE requires a positive latest sequence and birth height".into(),
        );
    }
    let expected_deadline = initial_birth_height
        .checked_add(terms.challenge_blocks)
        .ok_or("challenge deadline overflow")?;
    if challenge_deadline_height != expected_deadline {
        return Err(
            "challenge deadline does not equal initial birth height plus challenge_blocks".into(),
        );
    }
    let current = StateZero::new(terms).map_err(|error| error.to_string())?;
    let current_hash = current
        .hash(terms, &latest.funding_coin_id)
        .map_err(|error| error.to_string())?;
    let latest_hash = latest_checkpoint
        .hash(terms)
        .map_err(|error| error.to_string())?;
    let terms_hash = terms.hash().map_err(|error| error.to_string())?;
    let hashes = module_hashes();
    let current_commitment = closing_state_hash(
        &terms.network_id,
        &latest.funding_coin_id,
        &terms_hash,
        &[0; 8],
        &current_hash,
    );
    let current_puzzle_hash = one_arg_puzzle_hash(hashes.initial_closing, &current_commitment);
    let next_commitment = closing_state_hash(
        &terms.network_id,
        &latest.funding_coin_id,
        &terms_hash,
        &challenge_deadline_height.to_be_bytes(),
        &latest_hash,
    );
    let next_puzzle_hash = one_arg_puzzle_hash(hashes.subsequent_closing, &next_commitment);
    let solution = InitialClosingSolution {
        mode: 1,
        initial_birth_height: fixed(initial_birth_height),
        challenge_deadline_height: fixed(challenge_deadline_height),
        network_id: terms.network_id.into(),
        acceptance_blocks: fixed(terms.acceptance_blocks),
        freeze_blocks: fixed(terms.freeze_blocks),
        close_delay_blocks: fixed(terms.close_delay_blocks),
        challenge_blocks: fixed(terms.challenge_blocks),
        user_public_key: public_key(&terms.user_public_key, "user")?,
        hub_public_key: public_key(&terms.hub_state_public_key_a, "Hub")?,
        state_rules_hash: terms.state_rules_hash.into(),
        funding_amount: fixed(terms.funding_amount),
        user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
        max_ledger_entries: fixed(terms.max_ledger_entries),
        initial_closing_mod_hash: hashes.initial_closing.into(),
        subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
        payment_mod_hash: hashes.merchant_payment.into(),
        funding_coin_id: latest.funding_coin_id.into(),
        current_sequence: fixed(0),
        current_previous_checkpoint_hash: [0; 32].into(),
        current_manifest_root: current.manifest_root.into(),
        current_entry_count: fixed(0),
        current_reserved_total: fixed(0),
        current_user_remainder: fixed(current.user_remainder),
        current_entries: Vec::new(),
        new_sequence: fixed(latest_checkpoint.state_sequence),
        new_previous_checkpoint_hash: latest_checkpoint.previous_checkpoint_hash.into(),
        new_manifest_root: latest_checkpoint.manifest_root.into(),
        new_entry_count: fixed(latest_checkpoint.entry_count),
        new_reserved_total: fixed(latest_checkpoint.reserved_total),
        new_user_remainder: fixed(latest_checkpoint.user_remainder),
        new_entries: clvm_entries(&latest.entries)?,
    };
    let (cost, challenge_conditions, puzzle_reveal, encoded_solution) = run_curried_material(
        INITIAL_CLOSING_HEX,
        &ClosingArgs {
            current_commitment: current_commitment.into(),
        },
        &solution,
    )?;
    require_u64(
        &challenge_conditions,
        ASSERT_MY_BIRTH_HEIGHT,
        initial_birth_height,
    )?;
    require_u64(
        &challenge_conditions,
        ASSERT_BEFORE_HEIGHT_ABSOLUTE,
        challenge_deadline_height,
    )?;
    require_u64(
        &challenge_conditions,
        ASSERT_MY_AMOUNT,
        terms.funding_amount,
    )?;
    require_single_create(
        &challenge_conditions,
        &next_puzzle_hash,
        terms.funding_amount,
    )?;
    let expected_signatures = latest.entries.len() + 1;
    if conditions(&challenge_conditions, AGG_SIG_UNSAFE).len() != expected_signatures {
        return Err("State 0 CHALLENGE emitted an unexpected signature condition count".into());
    }
    let simulation = ChallengeSimulation {
        protocol_version: "0x0360".into(),
        funding_coin_id: hex::encode(latest.funding_coin_id),
        closing_coin_kind: ClosingCoinKind::Initial,
        current_state_sequence: 0,
        current_checkpoint_hash: hex::encode(current_hash),
        latest_state_sequence: latest_checkpoint.state_sequence,
        latest_checkpoint_hash: hex::encode(latest_hash),
        initial_birth_height,
        challenge_deadline_height,
        current_closing_puzzle_hash: hex::encode(current_puzzle_hash),
        next_closing_puzzle_hash: hex::encode(next_puzzle_hash),
        closing_amount_mojo: terms.funding_amount,
        cost,
        assert_my_birth_height: Some(initial_birth_height),
        assert_before_height_absolute: challenge_deadline_height,
        agg_sig_condition_count: expected_signatures as u64,
        recovery_packages_verified: true,
        all_clvm_conditions_verified: true,
        spend_bundle_created: false,
        broadcast_ready: false,
        chain_broadcast: false,
    };
    Ok(ChallengeSpendMaterial {
        simulation,
        puzzle_reveal,
        solution: encoded_solution,
        expected_closing_puzzle_hash: current_puzzle_hash,
        expected_next_closing_puzzle_hash: next_puzzle_hash,
        protocol_signatures: package_signatures(latest)?,
    })
}

fn fixed(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}

fn public_key(bytes: &[u8; 48], label: &str) -> Result<PublicKey, String> {
    PublicKey::from_bytes(bytes).map_err(|error| format!("invalid {label} public key: {error}"))
}

fn package_signatures(package: &RecoveryPackage) -> Result<Vec<Signature>, String> {
    let mut signatures = Vec::with_capacity(package.user_authorization_signatures.len() + 1);
    signatures.push(
        Signature::from_bytes(&package.official_state.hub_state_signature)
            .map_err(|error| format!("invalid Hub state signature: {error}"))?,
    );
    for signature in &package.user_authorization_signatures {
        signatures.push(
            Signature::from_bytes(signature)
                .map_err(|error| format!("invalid user authorization signature: {error}"))?,
        );
    }
    Ok(signatures)
}

fn clvm_entries(entries: &[LedgerEntry]) -> Result<Vec<ClvmEntry>, String> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            Ok(ClvmEntry {
                entry_index: fixed(index as u64),
                merchant_puzzle_hash: entry.merchant_puzzle_hash.into(),
                merchant_receipt_public_key: public_key(
                    &entry.merchant_receipt_public_key,
                    "merchant receipt",
                )?,
                amount: fixed(entry.amount),
                reservation_nonce: entry.reservation_nonce.into(),
            })
        })
        .collect()
}

fn list_nodes(allocator: &Allocator, mut node: NodePtr) -> Option<Vec<NodePtr>> {
    let mut items = Vec::new();
    loop {
        match allocator.sexp(node) {
            SExp::Pair(first, rest) => {
                items.push(first);
                node = rest;
            }
            SExp::Atom if allocator.atom(node).is_empty() => return Some(items),
            SExp::Atom => return None,
        }
    }
}

fn atom_u64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte))
}

fn read_conditions(allocator: &Allocator, root: NodePtr) -> Result<Vec<Condition>, String> {
    list_nodes(allocator, root)
        .ok_or("puzzle result is not a condition list")?
        .into_iter()
        .map(|condition| {
            let nodes = list_nodes(allocator, condition).ok_or("condition is not a list")?;
            let opcode = nodes.first().ok_or("condition omitted its opcode")?;
            Ok(Condition {
                opcode: atom_u64(allocator.atom(*opcode).as_ref()),
                args: nodes[1..]
                    .iter()
                    .map(|node| allocator.atom(*node).to_vec())
                    .collect(),
            })
        })
        .collect()
}

fn run_curried<A, S>(
    module_hex: &str,
    args: &A,
    solution: &S,
) -> Result<(u64, Vec<Condition>), String>
where
    A: ToClvm<Allocator>,
    S: ToClvm<Allocator>,
{
    let (cost, conditions, _, _) = run_curried_material(module_hex, args, solution)?;
    Ok((cost, conditions))
}

fn run_curried_material<A, S>(
    module_hex: &str,
    args: &A,
    solution: &S,
) -> Result<(u64, Vec<Condition>, Program, Program), String>
where
    A: ToClvm<Allocator>,
    S: ToClvm<Allocator>,
{
    let mut allocator = Allocator::new();
    let module = node_from_bytes(&mut allocator, &module_bytes(module_hex))
        .map_err(|error| error.to_string())?;
    let args = args
        .to_clvm(&mut allocator)
        .map_err(|error| format!("CLVM args: {error:?}"))?;
    let puzzle = CurriedProgram {
        program: module,
        args,
    }
    .to_clvm(&mut allocator)
    .map_err(|error| format!("CLVM curry: {error:?}"))?;
    let solution = solution
        .to_clvm(&mut allocator)
        .map_err(|error| format!("CLVM solution: {error:?}"))?;
    let reduction = run_puzzle_with_cost(&mut allocator, puzzle, solution, MAX_COST, false)
        .map_err(|error| {
            format!(
                "CLVM execution failed: {error:?}; puzzle_hash={}",
                hex::encode(crate::module_hash(
                    &node_to_bytes(&allocator, puzzle).unwrap_or_default()
                ))
            )
        })?;
    let puzzle_reveal = Program::from(
        node_to_bytes(&allocator, puzzle).map_err(|error| format!("serialize puzzle: {error}"))?,
    );
    let solution = Program::from(
        node_to_bytes(&allocator, solution)
            .map_err(|error| format!("serialize solution: {error}"))?,
    );
    Ok((
        reduction.0,
        read_conditions(&allocator, reduction.1)?,
        puzzle_reveal,
        solution,
    ))
}

fn conditions(all: &[Condition], opcode: u64) -> Vec<&Condition> {
    all.iter()
        .filter(|condition| condition.opcode == opcode)
        .collect()
}

fn require_u64(all: &[Condition], opcode: u64, expected: u64) -> Result<(), String> {
    let matches = conditions(all, opcode);
    if matches.len() != 1 || matches[0].args.len() != 1 || atom_u64(&matches[0].args[0]) != expected
    {
        return Err(format!("CLVM condition {opcode} does not equal {expected}"));
    }
    Ok(())
}

fn require_bytes(all: &[Condition], opcode: u64, expected: &[u8]) -> Result<(), String> {
    let matches = conditions(all, opcode);
    if matches.len() != 1 || matches[0].args.len() != 1 || matches[0].args[0] != expected {
        return Err(format!("CLVM condition {opcode} has an unexpected value"));
    }
    Ok(())
}

fn require_single_create(
    all: &[Condition],
    puzzle_hash: &[u8; 32],
    amount: u64,
) -> Result<(), String> {
    let creates = conditions(all, CREATE_COIN);
    if creates.len() != 1 {
        return Err("CLVM emitted an unexpected CREATE_COIN count".into());
    }
    require_create(creates[0], puzzle_hash, amount)
}

fn require_create(
    condition: &Condition,
    puzzle_hash: &[u8; 32],
    amount: u64,
) -> Result<(), String> {
    if condition.args.len() < 2
        || condition.args[0] != puzzle_hash
        || atom_u64(&condition.args[1]) != amount
    {
        return Err("CLVM CREATE_COIN output does not match the expected settlement".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chia_bls::SecretKey;
    use xhub_protocol_v3_6::{
        Ledger, OfficialState, RecoveryPackage, StateZero, public_key_bytes, sign_hash,
        state_rules_hash,
    };

    use super::*;
    use crate::funding_puzzle_reveal;

    fn package() -> RecoveryPackage {
        let user = SecretKey::from_seed(&[0x11; 32]);
        let hub = SecretKey::from_seed(&[0x22; 32]);
        let merchant = SecretKey::from_seed(&[0x33; 32]);
        let hashes = module_hashes();
        let terms = xhub_protocol_v3_6::ChannelTerms::new(
            [0xaa; 32],
            5,
            2,
            4,
            public_key_bytes(&user),
            public_key_bytes(&hub),
            state_rules_hash(
                &hashes.initial_closing,
                &hashes.subsequent_closing,
                &hashes.merchant_payment,
            ),
            10,
            [0xdd; 32],
        )
        .expect("terms");
        let funding_coin_id = [0xcc; 32];
        let entry = LedgerEntry {
            merchant_puzzle_hash: [0xee; 32],
            merchant_receipt_public_key: public_key_bytes(&merchant),
            amount: 1,
            reservation_nonce: [0x44; 32],
        };
        let state_zero = StateZero::new(&terms)
            .expect("state zero")
            .hash(&terms, &funding_coin_id)
            .expect("state zero hash");
        let checkpoint = Ledger {
            entries: vec![entry.clone()],
        }
        .checkpoint(&terms, funding_coin_id, 1, state_zero)
        .expect("checkpoint");
        let hub_hash = checkpoint.hub_state_hash(&terms).expect("hub hash");
        let (_, reveal) = funding_puzzle_reveal(&terms).expect("funding puzzle");
        RecoveryPackage {
            funding_coin_id,
            funding_puzzle_reveal: reveal.to_vec(),
            funding_amount: 10,
            channel_terms: terms.clone(),
            official_state: OfficialState {
                checkpoint,
                hub_state_signature: sign_hash(&hub, &hub_hash),
            },
            entries: vec![entry.clone()],
            user_authorization_signatures: vec![sign_hash(
                &user,
                &entry
                    .authorization_hash(&terms, &funding_coin_id)
                    .expect("authorization hash"),
            )],
        }
    }

    fn next_package(current: &RecoveryPackage) -> RecoveryPackage {
        let user = SecretKey::from_seed(&[0x11; 32]);
        let hub = SecretKey::from_seed(&[0x22; 32]);
        let merchant = SecretKey::from_seed(&[0x33; 32]);
        let mut entries = current.entries.clone();
        entries.push(LedgerEntry {
            merchant_puzzle_hash: [0xef; 32],
            merchant_receipt_public_key: public_key_bytes(&merchant),
            amount: 2,
            reservation_nonce: [0x45; 32],
        });
        let previous = current
            .official_state
            .checkpoint
            .hash(&current.channel_terms)
            .expect("current checkpoint hash");
        let checkpoint = Ledger {
            entries: entries.clone(),
        }
        .checkpoint(&current.channel_terms, current.funding_coin_id, 2, previous)
        .expect("next checkpoint");
        let signatures = entries
            .iter()
            .map(|entry| {
                sign_hash(
                    &user,
                    &entry
                        .authorization_hash(&current.channel_terms, &current.funding_coin_id)
                        .expect("authorization hash"),
                )
            })
            .collect();
        RecoveryPackage {
            funding_coin_id: current.funding_coin_id,
            funding_puzzle_reveal: current.funding_puzzle_reveal.clone(),
            funding_amount: current.funding_amount,
            channel_terms: current.channel_terms.clone(),
            official_state: OfficialState {
                hub_state_signature: sign_hash(
                    &hub,
                    &checkpoint
                        .hub_state_hash(&current.channel_terms)
                        .expect("Hub state hash"),
                ),
                checkpoint,
            },
            entries,
            user_authorization_signatures: signatures,
        }
    }

    #[test]
    fn simulates_signed_funding_finalize_and_merchant_forward() {
        let report = simulate_recovery_closing(&package(), 1_000).expect("simulation");
        assert_eq!(report.challenge_deadline_height, 1_004);
        assert_eq!(report.funding.assert_height_relative, 7);
        assert_eq!(report.initial_finalize.outputs.len(), 2);
        assert_eq!(report.initial_finalize.outputs[0].amount_mojo, 1);
        assert_eq!(report.initial_finalize.outputs[1].amount_mojo, 9);
        assert_eq!(report.merchant_forwards[0].forwarded_amount_mojo, 1);
        assert!(!report.broadcast_ready);
        assert!(!report.chain_broadcast);
    }

    #[test]
    fn rejects_a_tampered_recovery_package_before_clvm_execution() {
        let mut invalid = package();
        invalid.entries[0].amount = 2;
        assert!(simulate_recovery_closing(&invalid, 1_000).is_err());
    }

    #[test]
    fn simulates_initial_and_subsequent_challenges_without_a_spend_bundle() {
        let current = package();
        let latest = next_package(&current);
        for kind in [ClosingCoinKind::Initial, ClosingCoinKind::Subsequent] {
            let report = simulate_challenge(&current, &latest, kind, 1_000, 1_004)
                .expect("challenge simulation");
            assert_eq!(
                (report.current_state_sequence, report.latest_state_sequence),
                (1, 2)
            );
            assert_eq!(report.challenge_deadline_height, 1_004);
            assert_eq!(report.assert_before_height_absolute, 1_004);
            assert_eq!(report.agg_sig_condition_count, 5);
            assert_eq!(
                report.assert_my_birth_height,
                (kind == ClosingCoinKind::Initial).then_some(1_000)
            );
            assert!(!report.spend_bundle_created);
            assert!(!report.broadcast_ready);
            assert!(!report.chain_broadcast);
        }
    }

    #[test]
    fn rejects_stale_state_or_a_reset_challenge_deadline() {
        let current = package();
        let latest = next_package(&current);
        assert!(
            simulate_challenge(&latest, &current, ClosingCoinKind::Initial, 1_000, 1_004,).is_err()
        );
        assert!(
            simulate_challenge(&current, &latest, ClosingCoinKind::Subsequent, 1_000, 1_005,)
                .is_err()
        );
    }

    #[test]
    fn state_zero_can_be_challenged_by_the_first_complete_state() {
        let latest = package();
        let report =
            simulate_state_zero_challenge(&latest, 1_000, 1_004).expect("State 0 challenge");
        assert_eq!(
            (report.current_state_sequence, report.latest_state_sequence),
            (0, 1)
        );
        assert_eq!(report.agg_sig_condition_count, 2);
        assert_eq!(report.assert_my_birth_height, Some(1_000));
        assert!(!report.spend_bundle_created);
        assert!(!report.chain_broadcast);
    }
}
