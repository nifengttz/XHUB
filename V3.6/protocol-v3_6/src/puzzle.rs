use clvm_utils::{TreeHash, curry_tree_hash, tree_hash_atom};

use crate::{Bytes32, PROTOCOL_VERSION, sha256_parts};

pub const STATE_RULES_DOMAIN: &[u8] = b"XHUB_STATE_RULES_V3_6";
pub const CLOSING_STATE_DOMAIN: &[u8] = b"XHUB_CLOSING_STATE_V3_6";

pub fn state_rules_hash(
    initial_closing_mod_hash: &Bytes32,
    subsequent_closing_mod_hash: &Bytes32,
    merchant_payment_mod_hash: &Bytes32,
) -> Bytes32 {
    sha256_parts(&[
        STATE_RULES_DOMAIN,
        initial_closing_mod_hash,
        subsequent_closing_mod_hash,
        merchant_payment_mod_hash,
    ])
}

pub fn closing_state_hash(
    network_id: &Bytes32,
    funding_coin_id: &Bytes32,
    channel_terms_hash: &Bytes32,
    challenge_deadline_height: &[u8; 8],
    current_state_hash: &Bytes32,
) -> Bytes32 {
    sha256_parts(&[
        CLOSING_STATE_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        network_id,
        funding_coin_id,
        channel_terms_hash,
        challenge_deadline_height,
        current_state_hash,
    ])
}

pub fn one_arg_puzzle_hash(mod_hash: Bytes32, argument: &[u8]) -> Bytes32 {
    curry_tree_hash(TreeHash::new(mod_hash), &[tree_hash_atom(argument)]).to_bytes()
}

pub fn merchant_payment_puzzle_hash(
    merchant_payment_mod_hash: Bytes32,
    network_id: &Bytes32,
    funding_coin_id: &Bytes32,
    channel_terms_hash: &Bytes32,
    entry_index: u64,
    reservation_nonce: &Bytes32,
    merchant_puzzle_hash: &Bytes32,
) -> Bytes32 {
    let entry_index = entry_index.to_be_bytes();
    let argument_hashes = [
        tree_hash_atom(&PROTOCOL_VERSION.to_be_bytes()),
        tree_hash_atom(network_id),
        tree_hash_atom(funding_coin_id),
        tree_hash_atom(channel_terms_hash),
        tree_hash_atom(&entry_index),
        tree_hash_atom(reservation_nonce),
        tree_hash_atom(merchant_puzzle_hash),
    ];
    curry_tree_hash(TreeHash::new(merchant_payment_mod_hash), &argument_hashes).to_bytes()
}
