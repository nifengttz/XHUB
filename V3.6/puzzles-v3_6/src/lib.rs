use chia_protocol::{Bytes, Bytes32 as ChiaBytes32, Program};
use clvm_traits::ToClvm;
use clvm_utils::{CurriedProgram, TreeHash};
use clvmr::{
    Allocator,
    serde::{node_from_bytes, node_to_bytes},
};
use xhub_protocol_v3_6::{ChannelTerms, state_rules_hash};

mod closing;

pub use closing::*;

pub const FUNDING_HEX: &str = include_str!("../xhub_funding_v3_6.clsp.hex");
pub const INITIAL_CLOSING_HEX: &str = include_str!("../xhub_initial_closing_v3_6.clsp.hex");
pub const SUBSEQUENT_CLOSING_HEX: &str = include_str!("../xhub_subsequent_closing_v3_6.clsp.hex");
pub const MERCHANT_PAYMENT_HEX: &str = include_str!("../xhub_merchant_payment_v3_6.clsp.hex");

pub fn module_bytes(source: &str) -> Vec<u8> {
    hex::decode(source.trim()).expect("committed CLVM hex must be valid")
}

pub fn module_hash(bytes: &[u8]) -> [u8; 32] {
    use clvm_utils::tree_hash;
    use clvmr::{Allocator, serde::node_from_bytes};

    let mut allocator = Allocator::new();
    let node = node_from_bytes(&mut allocator, bytes).expect("committed CLVM must decode");
    let hash: TreeHash = tree_hash(&allocator, node);
    hash.to_bytes()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleHashes {
    pub funding: [u8; 32],
    pub initial_closing: [u8; 32],
    pub subsequent_closing: [u8; 32],
    pub merchant_payment: [u8; 32],
}

pub fn module_hashes() -> ModuleHashes {
    ModuleHashes {
        funding: module_hash(&module_bytes(FUNDING_HEX)),
        initial_closing: module_hash(&module_bytes(INITIAL_CLOSING_HEX)),
        subsequent_closing: module_hash(&module_bytes(SUBSEQUENT_CLOSING_HEX)),
        merchant_payment: module_hash(&module_bytes(MERCHANT_PAYMENT_HEX)),
    }
}

#[derive(Debug, Clone, ToClvm)]
#[clvm(curry)]
struct FundingArgs {
    network_id: ChiaBytes32,
    acceptance_blocks: Bytes,
    freeze_blocks: Bytes,
    close_delay_blocks: Bytes,
    challenge_blocks: Bytes,
    user_public_key: Bytes,
    hub_public_key: Bytes,
    state_rules_hash: ChiaBytes32,
    funding_amount: Bytes,
    user_remainder_puzzle_hash: ChiaBytes32,
    max_ledger_entries: Bytes,
    initial_closing_mod_hash: ChiaBytes32,
    subsequent_closing_mod_hash: ChiaBytes32,
    payment_mod_hash: ChiaBytes32,
}

fn fixed(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}

pub fn funding_puzzle_reveal(terms: &ChannelTerms) -> Result<(ChiaBytes32, Program), String> {
    terms.validate().map_err(|error| error.to_string())?;
    let hashes = module_hashes();
    let expected_state_rules = state_rules_hash(
        &hashes.initial_closing,
        &hashes.subsequent_closing,
        &hashes.merchant_payment,
    );
    if terms.state_rules_hash != expected_state_rules {
        return Err("state_rules_hash does not match the committed V3.6 modules".into());
    }

    let args = FundingArgs {
        network_id: terms.network_id.into(),
        acceptance_blocks: fixed(terms.acceptance_blocks),
        freeze_blocks: fixed(terms.freeze_blocks),
        close_delay_blocks: fixed(terms.close_delay_blocks),
        challenge_blocks: fixed(terms.challenge_blocks),
        user_public_key: terms.user_public_key.to_vec().into(),
        hub_public_key: terms.hub_state_public_key_a.to_vec().into(),
        state_rules_hash: terms.state_rules_hash.into(),
        funding_amount: fixed(terms.funding_amount),
        user_remainder_puzzle_hash: terms.user_remainder_puzzle_hash.into(),
        max_ledger_entries: fixed(terms.max_ledger_entries),
        initial_closing_mod_hash: hashes.initial_closing.into(),
        subsequent_closing_mod_hash: hashes.subsequent_closing.into(),
        payment_mod_hash: hashes.merchant_payment.into(),
    };

    let mut allocator = Allocator::new();
    let module = node_from_bytes(&mut allocator, &module_bytes(FUNDING_HEX))
        .map_err(|error| format!("cannot decode Funding module: {error}"))?;
    let puzzle = CurriedProgram {
        program: module,
        args: &args,
    }
    .to_clvm(&mut allocator)
    .map_err(|error| format!("cannot curry Funding module: {error:?}"))?;
    let puzzle_hash = ChiaBytes32::from(clvm_utils::tree_hash(&allocator, puzzle));
    let puzzle_reveal = Program::from(
        node_to_bytes(&allocator, puzzle)
            .map_err(|error| format!("cannot serialize Funding puzzle: {error}"))?,
    );
    Ok((puzzle_hash, puzzle_reveal))
}

pub fn funding_puzzle_hash(terms: &ChannelTerms) -> Result<[u8; 32], String> {
    Ok(funding_puzzle_reveal(terms)?.0.to_bytes())
}
