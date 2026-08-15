use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chia_bls::SecretKey;
use serde_json::{Value, json};
use tower::ServiceExt;
use xhub_hub_v3_6::{
    ChainPeak, ChainProviderError, ChainProviderResult, ChainSnapshot, ChainStateProvider,
    ChannelRegistration, FundingCoinState, HubStore,
    api::{
        ApiState, DeliveryTransportError, PROTOCOL_VERSION_HEADER, RecoveryPackageTransport, router,
    },
};
use xhub_protocol_v3_6::{ChannelTerms, LedgerEntry, RecoveryPackage, public_key_bytes, sign_hash};

const FUNDING_COIN_ID: [u8; 32] = [0x42; 32];

fn key(seed: u8) -> SecretKey {
    SecretKey::from_seed(&[seed; 32])
}

fn registration() -> ChannelRegistration {
    ChannelRegistration {
        funding_coin_id: FUNDING_COIN_ID,
        funding_puzzle_reveal: vec![0xff, 0x01, 0x80],
        funding_birth_height: 100,
        channel_terms: ChannelTerms::new(
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
        .expect("terms"),
    }
}

fn snapshot() -> ChainSnapshot {
    let registration = registration();
    ChainSnapshot {
        network_id: registration.channel_terms.network_id,
        synced: true,
        peak: Some(ChainPeak {
            height: 150,
            header_hash: [0x15; 32],
        }),
        funding_coin: FundingCoinState::Confirmed {
            birth_height: 100,
            puzzle_hash: registration.funding_puzzle_hash().expect("puzzle hash"),
            amount: registration.channel_terms.funding_amount,
        },
    }
}

struct ScriptedProvider {
    snapshots: Mutex<VecDeque<ChainProviderResult<ChainSnapshot>>>,
}

impl ChainStateProvider for ScriptedProvider {
    fn snapshot(&self, _funding_coin_id: [u8; 32]) -> ChainProviderResult<ChainSnapshot> {
        self.snapshots
            .lock()
            .expect("chain lock")
            .pop_front()
            .expect("scripted snapshot")
    }
}

#[derive(Default)]
struct RetryOnceTransport {
    attempts: Mutex<Vec<String>>,
}

impl RecoveryPackageTransport for RetryOnceTransport {
    fn deliver(
        &self,
        _recipient_id: &str,
        _recipient_kind: &str,
        idempotency_key: &str,
        _package: &RecoveryPackage,
    ) -> Result<(), DeliveryTransportError> {
        let mut attempts = self.attempts.lock().expect("delivery lock");
        attempts.push(idempotency_key.to_string());
        if attempts.len() == 1 {
            Err(DeliveryTransportError {
                retryable: true,
                message: "temporary timeout".into(),
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct QuorumTransport {
    attempts: Mutex<Vec<String>>,
}

impl RecoveryPackageTransport for QuorumTransport {
    fn recipient_ids(&self) -> Vec<String> {
        vec!["wt-a".into(), "wt-b".into(), "wt-c".into()]
    }

    fn deliver(
        &self,
        recipient_id: &str,
        _recipient_kind: &str,
        _idempotency_key: &str,
        _package: &RecoveryPackage,
    ) -> Result<(), DeliveryTransportError> {
        self.attempts
            .lock()
            .expect("delivery lock")
            .push(recipient_id.to_string());
        if recipient_id == "wt-c" {
            Err(DeliveryTransportError {
                retryable: true,
                message: "temporary wt-c outage".into(),
            })
        } else {
            Ok(())
        }
    }
}

fn signed_request_body(nonce: u8) -> Value {
    let registration = registration();
    let entry = LedgerEntry {
        merchant_puzzle_hash: [0x55; 32],
        merchant_receipt_public_key: public_key_bytes(&key(3)),
        amount: 100,
        reservation_nonce: [nonce; 32],
    };
    let hash = entry
        .authorization_hash(&registration.channel_terms, &FUNDING_COIN_ID)
        .expect("authorization hash");
    json!({
        "protocol_version": "0x0360",
        "request_id": hex::encode([0x88; 32]),
        "funding_coin_id": format!("0x{}", hex::encode(FUNDING_COIN_ID)),
        "merchant_puzzle_hash": hex::encode(entry.merchant_puzzle_hash),
        "merchant_receipt_public_key": hex::encode(entry.merchant_receipt_public_key),
        "amount": "100",
        "reservation_nonce": hex::encode(entry.reservation_nonce),
        "user_authorization_signature": hex::encode(sign_hash(&key(1), &hash)),
    })
}

fn app(transport: Arc<RetryOnceTransport>) -> Router {
    app_with_responses(transport, vec![Ok(snapshot()), Ok(snapshot())])
}

fn app_with_responses<T>(
    transport: Arc<T>,
    responses: Vec<ChainProviderResult<ChainSnapshot>>,
) -> Router
where
    T: RecoveryPackageTransport + 'static,
{
    let mut store = HubStore::open_in_memory().expect("store");
    store
        .register_channel(&registration(), 1_000)
        .expect("register");
    let provider = ScriptedProvider {
        snapshots: Mutex::new(responses.into()),
    };
    router(ApiState::new(store, Arc::new(provider), key(2), transport))
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(PROTOCOL_VERSION_HEADER, "0x0360");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).expect("JSON response"),
    )
}

#[tokio::test]
async fn authenticated_router_rejects_missing_or_wrong_tokens() {
    let state_app = app_with_responses(
        Arc::new(RetryOnceTransport::default()),
        vec![Ok(snapshot()), Ok(snapshot())],
    );
    let _ = state_app;

    let mut store = HubStore::open_in_memory().expect("store");
    store
        .register_channel(&registration(), 1_000)
        .expect("register");
    let provider = ScriptedProvider {
        snapshots: Mutex::new(VecDeque::new()),
    };
    let secured = xhub_hub_v3_6::api::authenticated_router(
        ApiState::new(
            store,
            Arc::new(provider),
            key(2),
            Arc::new(RetryOnceTransport::default()),
        ),
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
async fn reservation_status_uses_original_nonce_and_returns_same_signed_result() {
    let app = app(Arc::new(RetryOnceTransport::default()));
    let nonce = hex::encode([1; 32]);
    let (status, created) = call(
        &app,
        "POST",
        "/api/v3.6/reservations",
        Some(signed_request_body(1)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["result_class"], "SUCCESS");
    assert_eq!(created["client_action"], "ACCEPT");
    assert_eq!(created["status"], "SIGNED");
    assert_eq!(created["ledger_written"], true);
    assert!(created["signed_result_canonical_hex"].as_str().is_some());

    let (_, queried) = call(
        &app,
        "GET",
        &format!(
            "/api/v3.6/funding-coins/{}/reservations/{nonce}?protocol_version=0x0360",
            hex::encode(FUNDING_COIN_ID)
        ),
        None,
    )
    .await;
    assert_eq!(queried, created);

    let (_, unknown) = call(
        &app,
        "GET",
        &format!(
            "/api/v3.6/funding-coins/{}/reservations/{}?protocol_version=0x0360",
            hex::encode(FUNDING_COIN_ID),
            hex::encode([9; 32])
        ),
        None,
    )
    .await;
    assert_eq!(unknown["result_class"], "UNKNOWN");
    assert_eq!(unknown["client_action"], "RETRY_SAME_NONCE");
    assert!(unknown["ledger_written"].is_null());
}

#[tokio::test]
async fn recovery_delivery_retries_same_key_and_stops_after_success() {
    let transport = Arc::new(RetryOnceTransport::default());
    let app = app(Arc::clone(&transport));
    call(
        &app,
        "POST",
        "/api/v3.6/reservations",
        Some(signed_request_body(1)),
    )
    .await;

    let coin = hex::encode(FUNDING_COIN_ID);
    let (_, package) = call(
        &app,
        "GET",
        &format!("/api/v3.6/funding-coins/{coin}/recovery-packages/1?protocol_version=0x0360"),
        None,
    )
    .await;
    assert_eq!(package["state_sequence"], 1);
    assert!(package["recovery_package_canonical_hex"].as_str().is_some());

    let delivery = json!({
        "protocol_version": "0x0360",
        "recipient_id": "watchtower-1",
        "recipient_kind": "WATCHTOWER",
        "idempotency_key": "delivery-1"
    });
    let uri = format!("/api/v3.6/funding-coins/{coin}/recovery-packages/1/deliveries");
    let (first_status, first) = call(&app, "POST", &uri, Some(delivery.clone())).await;
    assert_eq!(first_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(first["delivery"]["status"], "FAILED_RETRYABLE");
    assert_eq!(first["delivery"]["attempt_count"], 1);

    let (second_status, second) = call(&app, "POST", &uri, Some(delivery.clone())).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second["delivery"]["status"], "DELIVERED");
    assert_eq!(second["delivery"]["attempt_count"], 2);

    let (_, third) = call(&app, "POST", &uri, Some(delivery)).await;
    assert_eq!(third["delivery"]["status"], "DELIVERED");
    assert_eq!(transport.attempts.lock().expect("attempts").len(), 2);

    let (_, listed) = call(&app, "GET", &format!("{uri}?protocol_version=0x0360"), None).await;
    assert_eq!(
        listed["deliveries"].as_array().expect("deliveries").len(),
        1
    );
    assert_eq!(
        listed["deliveries"][0]["recovery_package_content_hash"],
        package["recovery_package_content_hash"]
    );
}

#[tokio::test]
async fn watchtower_quorum_delivery_succeeds_with_two_of_three() {
    let transport = Arc::new(QuorumTransport::default());
    let app = app_with_responses(Arc::clone(&transport), vec![Ok(snapshot()), Ok(snapshot())]);
    call(
        &app,
        "POST",
        "/api/v3.6/reservations",
        Some(signed_request_body(1)),
    )
    .await;

    let coin = hex::encode(FUNDING_COIN_ID);
    let uri =
        format!("/api/v3.6/funding-coins/{coin}/recovery-packages/1/watchtower-quorum-deliveries");
    let request = json!({
        "protocol_version": "0x0360",
        "idempotency_key": "quorum-delivery-1"
    });
    let (status, body) = call(&app, "POST", &uri, Some(request.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured_recipient_count"], 3);
    assert_eq!(body["quorum_required"], 2);
    assert_eq!(body["delivered_count"], 2);
    assert_eq!(body["retryable_failure_count"], 1);
    assert_eq!(body["final_failure_count"], 0);
    assert_eq!(body["quorum_met"], true);
    assert_eq!(body["deliveries"][0]["recipient_id"], "wt-a");
    assert_eq!(body["deliveries"][1]["recipient_id"], "wt-b");
    assert_eq!(body["deliveries"][2]["recipient_id"], "wt-c");

    let (retry_status, retry) = call(&app, "POST", &uri, Some(request)).await;
    assert_eq!(retry_status, StatusCode::OK);
    assert_eq!(retry["delivered_count"], 2);
    assert_eq!(transport.attempts.lock().expect("attempts").len(), 4);
}

#[tokio::test]
async fn version_and_transport_encoding_are_strict() {
    let app = app(Arc::new(RetryOnceTransport::default()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v3.6/reservations")
                .header("content-type", "application/json")
                .body(Body::from(signed_request_body(1).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let mut malformed = signed_request_body(1);
    malformed["amount"] = json!("0100");
    let (status, body) = call(&app, "POST", "/api/v3.6/reservations", Some(malformed)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_AMOUNT");
    assert_eq!(body["ledger_written"], false);

    let mut invalid_bls = signed_request_body(1);
    invalid_bls["merchant_receipt_public_key"] = json!("00".repeat(48));
    let (status, body) = call(&app, "POST", "/api/v3.6/reservations", Some(invalid_bls)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_REQUEST");
    assert_eq!(body["client_action"], "STOP");
    assert_eq!(body["ledger_written"], false);
}

#[tokio::test]
async fn rpc_errors_remain_unknown_while_deterministic_rejections_are_final() {
    let rpc_error = || Err(ChainProviderError::RpcUnavailable("offline".into()));
    let rpc_app = app_with_responses(
        Arc::new(RetryOnceTransport::default()),
        vec![rpc_error(), rpc_error()],
    );
    let (status, unavailable) = call(
        &rpc_app,
        "POST",
        "/api/v3.6/reservations",
        Some(signed_request_body(1)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unavailable["status"], "RPC_UNAVAILABLE");
    assert_eq!(unavailable["result_class"], "UNKNOWN");
    assert_eq!(unavailable["client_action"], "RETRY_SAME_NONCE");
    assert!(unavailable["ledger_written"].is_null());
    assert!(
        unavailable["signed_result_canonical_hex"]
            .as_str()
            .is_some()
    );

    let app = app(Arc::new(RetryOnceTransport::default()));
    let mut invalid = signed_request_body(2);
    invalid["user_authorization_signature"] = json!(hex::encode(sign_hash(&key(9), &[3; 32])));
    let (_, rejected) = call(&app, "POST", "/api/v3.6/reservations", Some(invalid)).await;
    assert_eq!(rejected["status"], "INVALID_AUTHORIZATION");
    assert_eq!(rejected["result_class"], "REJECTED");
    assert_eq!(rejected["client_action"], "STOP");
    assert_eq!(rejected["ledger_written"], false);
    assert!(rejected["signed_result_canonical_hex"].as_str().is_some());
}
