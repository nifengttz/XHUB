use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chia_bls::SecretKey;
use serde_json::{Value, json};
use tower::ServiceExt;
use xhub_protocol_v3_6::{
    CanonicalEncode, ChannelTerms, Ledger, LedgerEntry, OfficialState, RecoveryPackage, StateZero,
    public_key_bytes, sign_hash,
};
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    api::{ApiState, PROTOCOL_VERSION_HEADER, router},
};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn package() -> RecoveryPackage {
    let terms = ChannelTerms::new(
        [0xaa; 32],
        100,
        10,
        50,
        public_key_bytes(&key(1)),
        public_key_bytes(&key(2)),
        [0x36; 32],
        1_000,
        [0x77; 32],
    )
    .expect("terms");
    let entry = LedgerEntry {
        merchant_puzzle_hash: [0x21; 32],
        merchant_receipt_public_key: public_key_bytes(&key(3)),
        amount: 100,
        reservation_nonce: [1; 32],
    };
    let zero = StateZero::new(&terms)
        .expect("zero")
        .hash(&terms, &FUNDING_COIN_ID)
        .expect("zero hash");
    let checkpoint = Ledger {
        entries: vec![entry.clone()],
    }
    .checkpoint(&terms, FUNDING_COIN_ID, 1, zero)
    .expect("checkpoint");
    RecoveryPackage {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_amount: terms.funding_amount,
        channel_terms: terms.clone(),
        official_state: OfficialState {
            checkpoint: checkpoint.clone(),
            hub_state_signature: sign_hash(
                &key(2),
                &checkpoint.hub_state_hash(&terms).expect("state hash"),
            ),
        },
        entries: vec![entry.clone()],
        user_authorization_signatures: vec![sign_hash(
            &key(1),
            &entry
                .authorization_hash(&terms, &FUNDING_COIN_ID)
                .expect("auth"),
        )],
    }
}

fn app() -> Router {
    let mut store = WatchtowerStore::open_in_memory().expect("store");
    store
        .register_confirmer("wt-1", "domain-a", public_key_bytes(&key(3)), 1)
        .expect("register");
    store
        .register_custody_attester("custody-1", "domain-b", public_key_bytes(&key(4)), 1)
        .expect("register custody one");
    store
        .register_custody_attester("custody-2", "domain-c", public_key_bytes(&key(5)), 1)
        .expect("register custody two");
    router(ApiState::new(store))
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(PROTOCOL_VERSION_HEADER, "0x0360");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).expect("json"))
}

#[tokio::test]
async fn authenticated_router_rejects_missing_or_wrong_tokens() {
    let secured = xhub_watchtower_v3_6::api::authenticated_router(
        ApiState::new(WatchtowerStore::open_in_memory().expect("store")),
        "correct-token-with-at-least-32-chars".into(),
    );
    for authorization in [None, Some("Bearer wrong-token-with-at-least-32-chars")] {
        let mut request = Request::get("/api/v3.6/health");
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response = secured
            .clone()
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = secured
        .oneshot(
            Request::get("/api/v3.6/health")
                .header(
                    "authorization",
                    "Bearer correct-token-with-at-least-32-chars",
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn package_ingest_and_greenlight_http_flow_is_versioned() {
    let app = app();
    let package = package();
    let bytes = hex::encode(package.canonical_bytes());
    let (status, accepted) = call(
        &app,
        "POST",
        "/api/v3.6/recovery-packages",
        Some(json!({"protocol_version":"0x0360", "recovery_package_canonical_hex": bytes})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["status"], "ACCEPTED");
    assert_eq!(accepted["state_sequence"], 1);

    let coin = hex::encode(FUNDING_COIN_ID);
    let (_, latest) = call(
        &app,
        "GET",
        &format!("/api/v3.6/funding-coins/{coin}/recovery-packages/latest?protocol_version=0x0360"),
        None,
    )
    .await;
    assert_eq!(
        latest["recovery_package_content_hash"],
        accepted["recovery_package_content_hash"]
    );

    let confirmation = xhub_protocol_v3_6::DeliveryConfirmation {
        network_id: package.channel_terms.network_id,
        funding_coin_id: package.funding_coin_id,
        channel_terms_hash: package.channel_terms.hash().expect("terms hash"),
        state_sequence: 1,
        checkpoint_hash: package
            .official_state
            .checkpoint
            .hash(&package.channel_terms)
            .expect("checkpoint hash"),
        entry_index: 0,
        authorization_hash: package.entries[0]
            .authorization_hash(&package.channel_terms, &package.funding_coin_id)
            .expect("auth"),
        recovery_package_content_hash: package.content_hash().expect("content hash"),
    };
    let signature = sign_hash(&key(3), &confirmation.hash().expect("confirmation hash"));
    let (status, recorded) = call(
        &app,
        "POST",
        "/api/v3.6/delivery-confirmations",
        Some(json!({
            "protocol_version":"0x0360", "signer_id":"wt-1", "failure_domain":"domain-a",
            "signer_public_key":hex::encode(public_key_bytes(&key(3))),
            "delivery_confirmation_canonical_hex":hex::encode(confirmation.canonical_bytes()),
            "signature":hex::encode(signature)
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recorded["status"], "ACCEPTED");

    let (_, greenlight) = call(&app, "GET", &format!("/api/v3.6/funding-coins/{coin}/states/1/entries/0/greenlight?protocol_version=0x0360&threshold=1"), None).await;
    assert_eq!(greenlight["delivered"], true);

    let (_, payload) = call(
        &app,
        "GET",
        &format!("/api/v3.6/funding-coins/{coin}/states/1/entries/0/custody-attestation?protocol_version=0x0360"),
        None,
    )
    .await;
    assert_eq!(payload["status"], "SIGNING_PAYLOAD");
    let attestation_hash: [u8; 32] = hex::decode(
        payload["custody_attestation_hash"]
            .as_str()
            .expect("attestation hash"),
    )
    .expect("hash hex")
    .try_into()
    .expect("32-byte hash");
    for (attester_id, domain, signer_key) in [
        ("custody-1", "domain-b", key(4)),
        ("custody-2", "domain-c", key(5)),
    ] {
        let signature = sign_hash(&signer_key, &attestation_hash);
        let (status, recorded) = call(
            &app,
            "POST",
            "/api/v3.6/custody-attestations",
            Some(json!({
                "protocol_version":"0x0360",
                "funding_coin_id":coin,
                "state_sequence":1,
                "entry_index":0,
                "attester_id":attester_id,
                "failure_domain":domain,
                "attester_public_key":hex::encode(public_key_bytes(&signer_key)),
                "signature":hex::encode(signature)
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recorded["status"], "ACCEPTED");
    }
    let (_, production) = call(
        &app,
        "GET",
        &format!("/api/v3.6/funding-coins/{coin}/states/1/entries/0/production-greenlight?protocol_version=0x0360&threshold=2"),
        None,
    )
    .await;
    assert_eq!(production["merchant_delivered"], true);
    assert_eq!(production["custody_attester_count"], 2);
    assert_eq!(production["custody_failure_domain_count"], 2);
    assert_eq!(production["production_ready"], true);

    let (_, single_vps) = call(
        &app,
        "GET",
        &format!("/api/v3.6/funding-coins/{coin}/states/1/entries/0/single-vps-test-greenlight?protocol_version=0x0360&threshold=2"),
        None,
    )
    .await;
    assert_eq!(single_vps["failure_domain_enforced"], false);
    assert_eq!(single_vps["test_only"], true);
    assert_eq!(single_vps["test_ready"], true);
    assert_eq!(single_vps["production_ready"], false);
}

#[tokio::test]
async fn missing_version_and_invalid_package_are_rejected_and_quarantined() {
    let app = app();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v3.6/recovery-packages")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"protocol_version":"0x0360", "recovery_package_canonical_hex":"00"})
                        .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (status, body) = call(
        &app,
        "POST",
        "/api/v3.6/recovery-packages",
        Some(json!({"protocol_version":"0x0360", "recovery_package_canonical_hex":"00"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_PACKAGE");
    assert_eq!(body["quarantined"], true);
}

#[tokio::test]
async fn execution_authorization_http_routes_are_versioned_and_fail_closed() {
    let app = app();
    let missing = hex::encode([0x55_u8; 32]);

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v3.6/execution-manifests/{missing}/authorization"),
        Some(json!({"protocol_version":"0x0360"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "AUTHORIZATION_NOT_AVAILABLE");

    let (status, body) = call(
        &app,
        "GET",
        &format!(
            "/api/v3.6/execution-authorizations/{missing}/simulated-receipt?protocol_version=0x0360"
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "SIMULATED_RECEIPT_NOT_FOUND");
    assert_eq!(body["accepted"], false);

    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/v3.6/execution-authorizations/{missing}?protocol_version=0x0360"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "AUTHORIZATION_NOT_FOUND");

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v3.6/execution-authorizations/{missing}/simulate"),
        Some(json!({
            "protocol_version":"0x0360",
            "submission_nonce":hex::encode([0x77_u8; 32])
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "AUTHORIZATION_NOT_AVAILABLE");

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v3.6/execution-manifests/{missing}/authorization"),
        Some(json!({"protocol_version":"0x9999"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "PROTOCOL_VERSION_MISMATCH");
}

#[tokio::test]
async fn execution_authorization_http_rejects_bundle_material_fields() {
    let app = app();
    let missing = hex::encode([0x66_u8; 32]);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v3.6/execution-manifests/{missing}/authorization"
                ))
                .header(PROTOCOL_VERSION_HEADER, "0x0360")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "protocol_version":"0x0360",
                        "spend_bundle_canonical_hex":"00"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn execution_audit_http_is_versioned_and_empty_chain_is_valid() {
    let app = app();
    let (status, body) = call(
        &app,
        "GET",
        "/api/v3.6/execution-audit?protocol_version=0x0360",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["event_count"], 0);
    assert_eq!(body["valid"], true);
    assert_eq!(body["broadcast_enabled"], false);
    assert_eq!(body["broadcast_ready"], false);
    assert_eq!(body["chain_broadcast"], false);
}

#[tokio::test]
async fn backup_retention_http_is_versioned_and_never_deletes() {
    let app = app();
    let manifest = json!({
        "backup_id": hex::encode([1_u8; 32]),
        "file_hash": hex::encode([2_u8; 32]),
        "size_bytes": 100,
        "audit_event_count": 0,
        "audit_head_hash": hex::encode([3_u8; 32]),
        "anchor_id": null,
        "created_at": 1,
    });
    let (status, body) = call(
        &app,
        "POST",
        "/api/v3.6/backup-retention-candidates",
        Some(json!({
            "protocol_version": "0x0360",
            "now": 100,
            "keep_latest": 1,
            "minimum_age_seconds": 10,
            "manifests": [manifest]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "BACKUP_NOT_AVAILABLE");

    let (status, body) = call(
        &app,
        "POST",
        "/api/v3.6/backup-retention-candidates",
        Some(json!({
            "protocol_version": "0x9999",
            "now": 100,
            "keep_latest": 1,
            "minimum_age_seconds": 10,
            "manifests": []
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "PROTOCOL_VERSION_MISMATCH");
}

#[tokio::test]
async fn backup_drill_query_is_versioned_and_fail_closed() {
    let app = app();
    let missing = hex::encode([0x91_u8; 32]);
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/v3.6/backup-restore-drills/{missing}?protocol_version=0x0360"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "BACKUP_DRILL_NOT_FOUND");
}
