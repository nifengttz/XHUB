use xhub_protocol_v3_6::state_rules_hash;
use xhub_puzzles_v3_6::module_hashes;
use xhub_wallet_v3_6::{FundingDraftStore, FundingTermsInput, WalletError};

const USER_KEY: &str = "89d0608036649d3484b7cfe71cfbd7f13015081d6206aede1aed0a4c1ad1521233123c08f0870e9d9f605ed429d24419";
const HUB_KEY: &str = "b61c4ee5d1cdd57ea615e6f3003e89afeee153d666562d0abec363d8b88c21c35e55f5622668b113e966564d04eb9fa1";

fn vector_input() -> FundingTermsInput {
    let modules = module_hashes();
    FundingTermsInput {
        network_id: "aa".repeat(32),
        acceptance_blocks: "12288".into(),
        freeze_blocks: "200".into(),
        challenge_blocks: "6000".into(),
        user_public_key: USER_KEY.into(),
        hub_state_public_key_a: HUB_KEY.into(),
        state_rules_hash: hex::encode(state_rules_hash(
            &modules.initial_closing,
            &modules.subsequent_closing,
            &modules.merchant_payment,
        )),
        funding_amount: "1000000".into(),
        user_remainder_puzzle_hash: "dd".repeat(32),
    }
}

#[test]
fn protocol_vector_defaults_produce_the_frozen_preview() {
    let terms = vector_input().to_channel_terms().expect("valid terms");
    let preview = xhub_wallet_v3_6::preview(&terms).expect("preview");

    assert_eq!(preview.close_delay_blocks, 12_488);
    assert_eq!(preview.funding_puzzle_hash.len(), 64);
    assert!(!preview.funding_puzzle_reveal.is_empty());
    assert_eq!(
        preview.funding_module_hash,
        "e2945105091602fb91db08af00525153604007791be6e673372e33880eb2e6ce"
    );
    assert_eq!(preview.funding_confirmation_blocks, 32);
    assert_eq!(preview.max_ledger_entries, 64);
    assert!(!preview.mainnet_approved);
}

#[test]
fn rejects_a_state_rules_hash_not_bound_to_the_committed_modules() {
    let mut input = vector_input();
    input.state_rules_hash = "bb".repeat(32);
    let terms = input.to_channel_terms().expect("structurally valid terms");
    assert!(matches!(
        xhub_wallet_v3_6::preview(&terms),
        Err(WalletError::Invalid(_))
    ));
}

#[test]
fn rejects_noncanonical_or_out_of_range_numbers() {
    for value in [
        "0",
        "-1",
        "01",
        "9223372036854775808",
        "18446744073709551616",
    ] {
        let mut input = vector_input();
        input.acceptance_blocks = value.into();
        assert!(
            matches!(input.to_channel_terms(), Err(WalletError::Invalid(_))),
            "accepted {value}"
        );
    }

    let mut overflow = vector_input();
    overflow.acceptance_blocks = "9223372036854775807".into();
    overflow.freeze_blocks = "1".into();
    assert!(overflow.to_channel_terms().is_err());
}

#[test]
fn rejects_invalid_bls_public_keys() {
    let mut input = vector_input();
    input.user_public_key = format!("c0{}", "00".repeat(47));
    assert!(input.to_channel_terms().is_err());
}

#[test]
fn confirmation_requires_the_server_computed_hash_and_is_idempotent() {
    let mut store = FundingDraftStore::default();
    let draft = store.prepare(&vector_input()).expect("prepare");
    assert!(matches!(
        store.confirm(&draft.draft_id, &"00".repeat(32)),
        Err(WalletError::ConfirmationMismatch)
    ));

    let confirmed = store
        .confirm(&draft.draft_id, &draft.preview.channel_terms_hash)
        .expect("confirm");
    assert!(confirmed.confirmed);
    assert_eq!(store.get(&draft.draft_id).expect("stored"), confirmed);
    assert_eq!(
        store
            .prepare(&vector_input())
            .expect("same immutable draft"),
        confirmed
    );

    let mut changed = vector_input();
    changed.challenge_blocks = "6001".into();
    let replacement = store.prepare(&changed).expect("new draft");
    assert_ne!(replacement.draft_id, confirmed.draft_id);
    assert!(!replacement.confirmed);
}
