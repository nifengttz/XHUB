use chia_bls::SecretKey;
use xhub_hub_v3_6::{ChannelRegistration, HubStore, ReservationRequest};
use xhub_protocol_v3_6::{
    CanonicalEncode, LedgerEntry, ReservationStatus, public_key_bytes, sign_hash,
};
use xhub_wallet_v3_6::FundingTermsInput;
use xhub_watchtower_v3_6::WatchtowerStore;

const FUNDING_COIN_ID: [u8; 32] = [0xcc; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn wallet_terms() -> xhub_protocol_v3_6::ChannelTerms {
    FundingTermsInput {
        network_id: "aa".repeat(32),
        acceptance_blocks: "12288".into(),
        freeze_blocks: "200".into(),
        challenge_blocks: "6000".into(),
        user_public_key: hex::encode(public_key_bytes(&key(0x11))),
        hub_state_public_key_a: hex::encode(public_key_bytes(&key(0x22))),
        state_rules_hash: "bb".repeat(32),
        funding_amount: "1000000".into(),
        user_remainder_puzzle_hash: "dd".repeat(32),
    }
    .to_channel_terms()
    .expect("wallet terms")
}

#[test]
fn wallet_hub_and_watchtower_share_consensus_hashes_end_to_end() {
    let terms = wallet_terms();
    let terms_hash = terms.hash().expect("terms hash");
    assert_eq!(
        hex::encode(terms_hash),
        "7586686e5a1432e9b0aa1511fd38fd71aafd2134df1f8c982cd07922bc93061f"
    );

    let registration = ChannelRegistration {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_birth_height: 50_000,
        channel_terms: terms.clone(),
    };
    let merchant_key = key(0x33);
    let entry = LedgerEntry {
        merchant_puzzle_hash: [0xe0; 32],
        merchant_receipt_public_key: public_key_bytes(&merchant_key),
        amount: 10_000,
        reservation_nonce: [1; 32],
    };
    let authorization_hash = entry
        .authorization_hash(&terms, &FUNDING_COIN_ID)
        .expect("authorization hash");
    let request = ReservationRequest {
        request_id: [0x44; 32],
        funding_coin_id: FUNDING_COIN_ID,
        ledger_entry: entry,
        user_authorization_signature: sign_hash(&key(0x11), &authorization_hash),
    };

    let mut hub = HubStore::open_in_memory().expect("hub");
    hub.register_channel(&registration, 1_000)
        .expect("register");
    let outcome = hub
        .reserve(&request, 50_001, &key(0x22), 1_001)
        .expect("reserve");
    assert_eq!(
        outcome.signed_result.result.status,
        ReservationStatus::Signed
    );
    let package = outcome.recovery_package.expect("recovery package");
    assert_eq!(
        package.channel_terms.hash().expect("package terms hash"),
        terms_hash
    );
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&terms)
        .expect("checkpoint hash");
    let content_hash = package.content_hash().expect("content hash");

    let mut watchtower = WatchtowerStore::open_in_memory().expect("watchtower");
    let accepted = watchtower
        .accept_package(&package.canonical_bytes(), 1_002)
        .expect("accept package");
    assert_eq!(accepted.checkpoint_hash, checkpoint_hash);
    assert_eq!(accepted.recovery_package_content_hash, content_hash);

    watchtower
        .register_confirmer(
            "merchant-1",
            "domain-a",
            public_key_bytes(&merchant_key),
            1_003,
        )
        .expect("register confirmer");
    let confirmation = watchtower
        .sign_confirmation(FUNDING_COIN_ID, 1, 0, "merchant-1", &merchant_key)
        .expect("sign confirmation");
    assert_eq!(confirmation.confirmation.channel_terms_hash, terms_hash);
    assert_eq!(confirmation.confirmation.checkpoint_hash, checkpoint_hash);
    assert_eq!(
        confirmation.confirmation.recovery_package_content_hash,
        content_hash
    );
    assert_eq!(
        confirmation.confirmation.authorization_hash,
        authorization_hash
    );
    watchtower
        .record_confirmation(&confirmation, 1_004)
        .expect("record confirmation");
    let greenlight = watchtower
        .greenlight_status(FUNDING_COIN_ID, 1, 0, 1)
        .expect("greenlight");
    assert!(greenlight.delivered);
    assert_eq!(
        (greenlight.signer_count, greenlight.failure_domain_count),
        (1, 1)
    );
}
