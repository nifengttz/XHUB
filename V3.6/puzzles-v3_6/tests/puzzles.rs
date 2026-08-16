use std::fs;

use chia_bls::{PublicKey, SecretKey};
use chia_protocol::{Bytes, Bytes32};
use chia_sdk_types::run_puzzle_with_cost;
use clvm_traits::ToClvm;
use clvm_utils::{CurriedProgram, tree_hash};
use clvmr::{
    Allocator, NodePtr, SExp,
    serde::{node_from_bytes, node_to_bytes},
};
use serde_json::Value;
use xhub_protocol_v3_6::{
    ChannelTerms, Ledger, LedgerEntry, StateZero, closing_state_hash, merchant_payment_puzzle_hash,
    one_arg_puzzle_hash, state_rules_hash,
};
use xhub_puzzles_v3_6::{
    FUNDING_HEX, INITIAL_CLOSING_HEX, MERCHANT_PAYMENT_HEX, SUBSEQUENT_CLOSING_HEX, module_bytes,
    module_hash,
};

const MAX_COST: u64 = 11_000_000_000;

fn fixed(value: u64) -> Bytes {
    value.to_be_bytes().to_vec().into()
}

fn bytes32(value: [u8; 32]) -> Bytes32 {
    value.into()
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

#[derive(Debug, Clone)]
struct StateData {
    sequence: u64,
    previous_checkpoint_hash: [u8; 32],
    manifest_root: [u8; 32],
    reserved_total: u64,
    user_remainder: u64,
    entries: Vec<LedgerEntry>,
    state_hash: [u8; 32],
}

impl StateData {
    fn clvm_entries(&self) -> Vec<ClvmEntry> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| ClvmEntry {
                entry_index: fixed(index as u64),
                merchant_puzzle_hash: bytes32(entry.merchant_puzzle_hash),
                merchant_receipt_public_key: PublicKey::from_bytes(
                    &entry.merchant_receipt_public_key,
                )
                .expect("fixture merchant key"),
                amount: fixed(entry.amount),
                reservation_nonce: bytes32(entry.reservation_nonce),
            })
            .collect()
    }

    fn funding_solution(&self, funding_coin_id: [u8; 32]) -> FundingSolution {
        FundingSolution {
            funding_coin_id: bytes32(funding_coin_id),
            state_sequence: fixed(self.sequence),
            previous_checkpoint_hash: bytes32(self.previous_checkpoint_hash),
            manifest_root: bytes32(self.manifest_root),
            entry_count: fixed(self.entries.len() as u64),
            reserved_total: fixed(self.reserved_total),
            user_remainder: fixed(self.user_remainder),
            entries: self.clvm_entries(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Condition {
    opcode: u64,
    args: Vec<Vec<u8>>,
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

fn read_conditions(allocator: &Allocator, root: NodePtr) -> Vec<Condition> {
    list_nodes(allocator, root)
        .expect("puzzle result must be a condition list")
        .into_iter()
        .map(|condition| {
            let nodes = list_nodes(allocator, condition).expect("condition must be a list");
            let opcode = atom_u64(allocator.atom(nodes[0]).as_ref());
            let args = nodes[1..]
                .iter()
                .map(|node| allocator.atom(*node).to_vec())
                .collect();
            Condition { opcode, args }
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
    let mut allocator = Allocator::new();
    let module = node_from_bytes(&mut allocator, &module_bytes(module_hex))
        .map_err(|error| error.to_string())?;
    let args = args
        .to_clvm(&mut allocator)
        .map_err(|error| format!("args: {error:?}"))?;
    let puzzle = CurriedProgram {
        program: module,
        args,
    }
    .to_clvm(&mut allocator)
    .map_err(|error| format!("curry: {error:?}"))?;
    let solution = solution
        .to_clvm(&mut allocator)
        .map_err(|error| format!("solution: {error:?}"))?;
    let reduction = run_puzzle_with_cost(&mut allocator, puzzle, solution, MAX_COST, false)
        .map_err(|error| {
            format!(
                "{error:?}; puzzle={}; solution={}",
                hex::encode(node_to_bytes(&allocator, puzzle).expect("serialize puzzle")),
                hex::encode(node_to_bytes(&allocator, solution).expect("serialize solution"))
            )
        })?;
    Ok((reduction.0, read_conditions(&allocator, reduction.1)))
}

fn condition(conditions: &[Condition], opcode: u64) -> Vec<&Condition> {
    conditions
        .iter()
        .filter(|condition| condition.opcode == opcode)
        .collect()
}

#[derive(Clone)]
struct Fixture {
    user_secret: SecretKey,
    hub_secret: SecretKey,
    merchant_secret: SecretKey,
    terms: ChannelTerms,
    funding_coin_id: [u8; 32],
    initial_mod_hash: [u8; 32],
    subsequent_mod_hash: [u8; 32],
    payment_mod_hash: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        let user_secret = SecretKey::from_seed(&[0x11; 32]);
        let hub_secret = SecretKey::from_seed(&[0x22; 32]);
        let merchant_secret = SecretKey::from_seed(&[0x33; 32]);
        let initial_mod_hash = module_hash(&module_bytes(INITIAL_CLOSING_HEX));
        let subsequent_mod_hash = module_hash(&module_bytes(SUBSEQUENT_CLOSING_HEX));
        let payment_mod_hash = module_hash(&module_bytes(MERCHANT_PAYMENT_HEX));
        let state_rules =
            state_rules_hash(&initial_mod_hash, &subsequent_mod_hash, &payment_mod_hash);
        let terms = ChannelTerms::new(
            [0xaa; 32],
            5,
            2,
            4,
            user_secret.public_key().to_bytes(),
            hub_secret.public_key().to_bytes(),
            state_rules,
            1_000_000,
            [0xdd; 32],
        )
        .expect("fixture terms");
        Self {
            user_secret,
            hub_secret,
            merchant_secret,
            terms,
            funding_coin_id: [0xcc; 32],
            initial_mod_hash,
            subsequent_mod_hash,
            payment_mod_hash,
        }
    }

    fn funding_args(&self) -> FundingArgs {
        FundingArgs {
            network_id: bytes32(self.terms.network_id),
            acceptance_blocks: fixed(self.terms.acceptance_blocks),
            freeze_blocks: fixed(self.terms.freeze_blocks),
            close_delay_blocks: fixed(self.terms.close_delay_blocks),
            challenge_blocks: fixed(self.terms.challenge_blocks),
            user_public_key: self.user_secret.public_key(),
            hub_public_key: self.hub_secret.public_key(),
            state_rules_hash: bytes32(self.terms.state_rules_hash),
            funding_amount: fixed(self.terms.funding_amount),
            user_remainder_puzzle_hash: bytes32(self.terms.user_remainder_puzzle_hash),
            max_ledger_entries: fixed(self.terms.max_ledger_entries),
            initial_closing_mod_hash: bytes32(self.initial_mod_hash),
            subsequent_closing_mod_hash: bytes32(self.subsequent_mod_hash),
            payment_mod_hash: bytes32(self.payment_mod_hash),
        }
    }

    fn entries(&self, count: usize) -> Vec<LedgerEntry> {
        (0..count)
            .map(|index| {
                let mut nonce = [0_u8; 32];
                nonce[24..].copy_from_slice(&(index as u64 + 1).to_be_bytes());
                LedgerEntry {
                    merchant_puzzle_hash: [0xe1; 32],
                    merchant_receipt_public_key: self.merchant_secret.public_key().to_bytes(),
                    amount: 1_000 + index as u64,
                    reservation_nonce: nonce,
                }
            })
            .collect()
    }

    fn state_zero(&self) -> StateData {
        let state = StateZero::new(&self.terms).expect("state zero");
        StateData {
            sequence: 0,
            previous_checkpoint_hash: [0; 32],
            manifest_root: state.manifest_root,
            reserved_total: 0,
            user_remainder: state.user_remainder,
            entries: Vec::new(),
            state_hash: state
                .hash(&self.terms, &self.funding_coin_id)
                .expect("state zero hash"),
        }
    }

    fn state(&self, count: usize, sequence: u64, previous: [u8; 32]) -> StateData {
        let entries = self.entries(count);
        let ledger = Ledger {
            entries: entries.clone(),
        };
        let checkpoint = ledger
            .checkpoint(&self.terms, self.funding_coin_id, sequence, previous)
            .expect("checkpoint");
        let state_hash = checkpoint.hash(&self.terms).expect("checkpoint hash");
        StateData {
            sequence,
            previous_checkpoint_hash: previous,
            manifest_root: checkpoint.manifest_root,
            reserved_total: checkpoint.reserved_total,
            user_remainder: checkpoint.user_remainder,
            entries,
            state_hash,
        }
    }

    fn initial_solution(
        &self,
        mode: u8,
        birth_height: u64,
        deadline: u64,
        current: &StateData,
        new: &StateData,
    ) -> InitialClosingSolution {
        InitialClosingSolution {
            mode,
            initial_birth_height: fixed(birth_height),
            challenge_deadline_height: fixed(deadline),
            network_id: bytes32(self.terms.network_id),
            acceptance_blocks: fixed(self.terms.acceptance_blocks),
            freeze_blocks: fixed(self.terms.freeze_blocks),
            close_delay_blocks: fixed(self.terms.close_delay_blocks),
            challenge_blocks: fixed(self.terms.challenge_blocks),
            user_public_key: self.user_secret.public_key(),
            hub_public_key: self.hub_secret.public_key(),
            state_rules_hash: bytes32(self.terms.state_rules_hash),
            funding_amount: fixed(self.terms.funding_amount),
            user_remainder_puzzle_hash: bytes32(self.terms.user_remainder_puzzle_hash),
            max_ledger_entries: fixed(self.terms.max_ledger_entries),
            initial_closing_mod_hash: bytes32(self.initial_mod_hash),
            subsequent_closing_mod_hash: bytes32(self.subsequent_mod_hash),
            payment_mod_hash: bytes32(self.payment_mod_hash),
            funding_coin_id: bytes32(self.funding_coin_id),
            current_sequence: fixed(current.sequence),
            current_previous_checkpoint_hash: bytes32(current.previous_checkpoint_hash),
            current_manifest_root: bytes32(current.manifest_root),
            current_entry_count: fixed(current.entries.len() as u64),
            current_reserved_total: fixed(current.reserved_total),
            current_user_remainder: fixed(current.user_remainder),
            current_entries: current.clvm_entries(),
            new_sequence: fixed(new.sequence),
            new_previous_checkpoint_hash: bytes32(new.previous_checkpoint_hash),
            new_manifest_root: bytes32(new.manifest_root),
            new_entry_count: fixed(new.entries.len() as u64),
            new_reserved_total: fixed(new.reserved_total),
            new_user_remainder: fixed(new.user_remainder),
            new_entries: new.clvm_entries(),
        }
    }

    fn subsequent_solution(
        &self,
        mode: u8,
        deadline: u64,
        current: &StateData,
        new: &StateData,
    ) -> SubsequentClosingSolution {
        SubsequentClosingSolution {
            mode,
            challenge_deadline_height: fixed(deadline),
            network_id: bytes32(self.terms.network_id),
            acceptance_blocks: fixed(self.terms.acceptance_blocks),
            freeze_blocks: fixed(self.terms.freeze_blocks),
            close_delay_blocks: fixed(self.terms.close_delay_blocks),
            challenge_blocks: fixed(self.terms.challenge_blocks),
            user_public_key: self.user_secret.public_key(),
            hub_public_key: self.hub_secret.public_key(),
            state_rules_hash: bytes32(self.terms.state_rules_hash),
            funding_amount: fixed(self.terms.funding_amount),
            user_remainder_puzzle_hash: bytes32(self.terms.user_remainder_puzzle_hash),
            max_ledger_entries: fixed(self.terms.max_ledger_entries),
            initial_closing_mod_hash: bytes32(self.initial_mod_hash),
            subsequent_closing_mod_hash: bytes32(self.subsequent_mod_hash),
            payment_mod_hash: bytes32(self.payment_mod_hash),
            funding_coin_id: bytes32(self.funding_coin_id),
            current_sequence: fixed(current.sequence),
            current_previous_checkpoint_hash: bytes32(current.previous_checkpoint_hash),
            current_manifest_root: bytes32(current.manifest_root),
            current_entry_count: fixed(current.entries.len() as u64),
            current_reserved_total: fixed(current.reserved_total),
            current_user_remainder: fixed(current.user_remainder),
            current_entries: current.clvm_entries(),
            new_sequence: fixed(new.sequence),
            new_previous_checkpoint_hash: bytes32(new.previous_checkpoint_hash),
            new_manifest_root: bytes32(new.manifest_root),
            new_entry_count: fixed(new.entries.len() as u64),
            new_reserved_total: fixed(new.reserved_total),
            new_user_remainder: fixed(new.user_remainder),
            new_entries: new.clvm_entries(),
        }
    }

    fn initial_commitment(&self, current: &StateData) -> [u8; 32] {
        closing_state_hash(
            &self.terms.network_id,
            &self.funding_coin_id,
            &self.terms.hash().expect("terms hash"),
            &[0; 8],
            &current.state_hash,
        )
    }

    fn subsequent_commitment(&self, deadline: u64, current: &StateData) -> [u8; 32] {
        closing_state_hash(
            &self.terms.network_id,
            &self.funding_coin_id,
            &self.terms.hash().expect("terms hash"),
            &deadline.to_be_bytes(),
            &current.state_hash,
        )
    }
}

#[test]
fn committed_module_hashes_match_compiled_programs() {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/module-hashes.json");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(manifest_path).expect("module hash manifest must exist"),
    )
    .expect("module hash manifest must be JSON");
    let modules = [
        ("xhub_funding_v3_6", FUNDING_HEX),
        ("xhub_initial_closing_v3_6", INITIAL_CLOSING_HEX),
        ("xhub_subsequent_closing_v3_6", SUBSEQUENT_CLOSING_HEX),
        ("xhub_merchant_payment_v3_6", MERCHANT_PAYMENT_HEX),
    ];
    for (name, source) in modules {
        assert_eq!(
            hex::encode(module_hash(&module_bytes(source))),
            manifest["modules"][name]["module_hash"]
        );
    }
}

#[test]
fn funding_state_zero_emits_relative_height_and_initial_closing_coin() {
    let fixture = Fixture::new();
    let state_zero = fixture.state_zero();
    let args = fixture.funding_args();
    let solution = state_zero.funding_solution(fixture.funding_coin_id);
    let (_, conditions) = run_curried(FUNDING_HEX, &args, &solution).expect("funding state zero");

    assert_eq!(atom_u64(&condition(&conditions, 82)[0].args[0]), 7);
    assert_eq!(atom_u64(&condition(&conditions, 73)[0].args[0]), 1_000_000);
    assert!(condition(&conditions, 49).is_empty());
    let expected_puzzle_hash = one_arg_puzzle_hash(
        fixture.initial_mod_hash,
        &fixture.initial_commitment(&state_zero),
    );
    let create = condition(&conditions, 51);
    assert_eq!(create.len(), 1);
    assert_eq!(create[0].args[0], expected_puzzle_hash);
    assert_eq!(atom_u64(&create[0].args[1]), fixture.terms.funding_amount);

    let mut invalid_args = args.clone();
    invalid_args.close_delay_blocks = fixed(6);
    assert!(run_curried(FUNDING_HEX, &invalid_args, &solution).is_err());
}

#[test]
fn funding_validates_full_ledgers_at_required_sizes() {
    let fixture = Fixture::new();
    let state_zero = fixture.state_zero();
    let args = fixture.funding_args();
    let mut costs = Vec::new();

    for count in [1_usize, 10, 64] {
        let state = fixture.state(count, 1, state_zero.state_hash);
        let solution = state.funding_solution(fixture.funding_coin_id);
        let (cost, conditions) =
            run_curried(FUNDING_HEX, &args, &solution).expect("funding full ledger");
        assert_eq!(condition(&conditions, 49).len(), count + 1);
        assert_eq!(condition(&conditions, 51).len(), 1);
        costs.push((count, cost));
    }
    assert!(costs.windows(2).all(|pair| pair[0].1 < pair[1].1));

    let mut tampered = fixture
        .state(1, 1, state_zero.state_hash)
        .funding_solution(fixture.funding_coin_id);
    tampered.manifest_root = bytes32([0x99; 32]);
    assert!(run_curried(FUNDING_HEX, &args, &tampered).is_err());
}

#[test]
fn initial_challenge_derives_deadline_from_birth_height() {
    let fixture = Fixture::new();
    let state_zero = fixture.state_zero();
    let state_one = fixture.state(1, 1, state_zero.state_hash);
    let commitment = fixture.initial_commitment(&state_zero);
    let args = ClosingArgs {
        current_commitment: bytes32(commitment),
    };
    let solution = fixture.initial_solution(1, 100, 104, &state_zero, &state_one);
    let (_, conditions) =
        run_curried(INITIAL_CLOSING_HEX, &args, &solution).expect("initial challenge");

    assert_eq!(atom_u64(&condition(&conditions, 75)[0].args[0]), 100);
    assert_eq!(atom_u64(&condition(&conditions, 87)[0].args[0]), 104);
    assert_eq!(condition(&conditions, 49).len(), 2);
    let expected_commitment = fixture.subsequent_commitment(104, &state_one);
    let expected_puzzle_hash =
        one_arg_puzzle_hash(fixture.subsequent_mod_hash, &expected_commitment);
    let creates = condition(&conditions, 51);
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0].args[0], expected_puzzle_hash);

    let bad_deadline = fixture.initial_solution(1, 100, 105, &state_zero, &state_one);
    assert!(run_curried(INITIAL_CLOSING_HEX, &args, &bad_deadline).is_err());
}

#[test]
fn subsequent_challenge_keeps_deadline_and_requires_higher_sequence() {
    let fixture = Fixture::new();
    let state_zero = fixture.state_zero();
    let state_one = fixture.state(1, 1, state_zero.state_hash);
    let state_two = fixture.state(2, 2, state_one.state_hash);
    let args = ClosingArgs {
        current_commitment: bytes32(fixture.subsequent_commitment(104, &state_one)),
    };
    let solution = fixture.subsequent_solution(1, 104, &state_one, &state_two);
    let (_, conditions) =
        run_curried(SUBSEQUENT_CLOSING_HEX, &args, &solution).expect("subsequent challenge");

    assert!(condition(&conditions, 75).is_empty());
    assert_eq!(atom_u64(&condition(&conditions, 87)[0].args[0]), 104);
    let expected = one_arg_puzzle_hash(
        fixture.subsequent_mod_hash,
        &fixture.subsequent_commitment(104, &state_two),
    );
    assert_eq!(condition(&conditions, 51)[0].args[0], expected);

    let stale = fixture.subsequent_solution(1, 104, &state_one, &state_one);
    assert!(run_curried(SUBSEQUENT_CLOSING_HEX, &args, &stale).is_err());
}

#[test]
fn finalize_creates_unique_payment_coins_and_remainder() {
    let fixture = Fixture::new();
    let state_zero = fixture.state_zero();
    let state = fixture.state(2, 1, state_zero.state_hash);
    let args = ClosingArgs {
        current_commitment: bytes32(fixture.subsequent_commitment(104, &state)),
    };
    let solution = fixture.subsequent_solution(2, 104, &state, &state);
    let (_, conditions) =
        run_curried(SUBSEQUENT_CLOSING_HEX, &args, &solution).expect("subsequent finalize");

    assert_eq!(atom_u64(&condition(&conditions, 83)[0].args[0]), 104);
    let creates = condition(&conditions, 51);
    assert_eq!(creates.len(), 3);
    let terms_hash = fixture.terms.hash().expect("terms hash");
    for (index, entry) in state.entries.iter().enumerate() {
        let expected = merchant_payment_puzzle_hash(
            fixture.payment_mod_hash,
            &fixture.terms.network_id,
            &fixture.funding_coin_id,
            &terms_hash,
            index as u64,
            &entry.reservation_nonce,
            &entry.merchant_puzzle_hash,
        );
        assert_eq!(creates[index].args[0], expected);
        assert_eq!(atom_u64(&creates[index].args[1]), entry.amount);
    }
    assert_ne!(creates[0].args[0], creates[1].args[0]);
    assert_eq!(creates[2].args[0], fixture.terms.user_remainder_puzzle_hash);
    assert_eq!(atom_u64(&creates[2].args[1]), state.user_remainder);
}

#[test]
fn merchant_payment_forwards_the_full_coin_amount() {
    let fixture = Fixture::new();
    let state = fixture.state(1, 1, fixture.state_zero().state_hash);
    let entry = &state.entries[0];
    let terms_hash = fixture.terms.hash().expect("terms hash");
    let args = PaymentArgs {
        protocol_version: vec![0x03, 0x60].into(),
        network_id: bytes32(fixture.terms.network_id),
        funding_coin_id: bytes32(fixture.funding_coin_id),
        channel_terms_hash: bytes32(terms_hash),
        entry_index: fixed(0),
        reservation_nonce: bytes32(entry.reservation_nonce),
        merchant_puzzle_hash: bytes32(entry.merchant_puzzle_hash),
    };
    let solution = PaymentSolution {
        payment_coin_amount: fixed(entry.amount),
    };
    let (_, conditions) =
        run_curried(MERCHANT_PAYMENT_HEX, &args, &solution).expect("payment forward");
    assert_eq!(
        atom_u64(&condition(&conditions, 73)[0].args[0]),
        entry.amount
    );
    let create = condition(&conditions, 51)[0];
    assert_eq!(create.args[0], entry.merchant_puzzle_hash);
    assert_eq!(atom_u64(&create.args[1]), entry.amount);

    let zero = PaymentSolution {
        payment_coin_amount: fixed(0),
    };
    assert!(run_curried(MERCHANT_PAYMENT_HEX, &args, &zero).is_err());
}

#[test]
fn curry_hash_helpers_match_real_clvm_programs() {
    let fixture = Fixture::new();
    let state = fixture.state(1, 1, fixture.state_zero().state_hash);
    let entry = &state.entries[0];
    let terms_hash = fixture.terms.hash().expect("terms hash");
    let args = PaymentArgs {
        protocol_version: vec![0x03, 0x60].into(),
        network_id: bytes32(fixture.terms.network_id),
        funding_coin_id: bytes32(fixture.funding_coin_id),
        channel_terms_hash: bytes32(terms_hash),
        entry_index: fixed(0),
        reservation_nonce: bytes32(entry.reservation_nonce),
        merchant_puzzle_hash: bytes32(entry.merchant_puzzle_hash),
    };
    let mut allocator = Allocator::new();
    let module = node_from_bytes(&mut allocator, &module_bytes(MERCHANT_PAYMENT_HEX))
        .expect("payment module");
    let encoded_args = args.to_clvm(&mut allocator).expect("payment args");
    let puzzle = CurriedProgram {
        program: module,
        args: encoded_args,
    }
    .to_clvm(&mut allocator)
    .expect("payment curry");
    let actual: [u8; 32] = tree_hash(&allocator, puzzle).into();
    let expected = merchant_payment_puzzle_hash(
        fixture.payment_mod_hash,
        &fixture.terms.network_id,
        &fixture.funding_coin_id,
        &terms_hash,
        0,
        &entry.reservation_nonce,
        &entry.merchant_puzzle_hash,
    );
    assert_eq!(actual, expected);
}
