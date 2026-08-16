use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn prepare_body() -> Value {
    let modules = xhub_puzzles_v3_6::module_hashes();
    let state_rules_hash = xhub_protocol_v3_6::state_rules_hash(
        &modules.initial_closing,
        &modules.subsequent_closing,
        &modules.merchant_payment,
    );
    json!({
        "protocol_version": "0x0360",
        "network_id": xhub_wallet_v3_6::MAINNET_NETWORK_ID,
        "acceptance_blocks": "12288",
        "freeze_blocks": "200",
        "challenge_blocks": "6000",
        "user_public_key": "89d0608036649d3484b7cfe71cfbd7f13015081d6206aede1aed0a4c1ad1521233123c08f0870e9d9f605ed429d24419",
        "hub_state_public_key_a": xhub_wallet_v3_6::api::DEFAULT_HUB_STATE_PUBLIC_KEY_A,
        "state_rules_hash": hex::encode(state_rules_hash),
        "funding_amount": "1000000",
        "user_remainder_puzzle_hash": "dd".repeat(32)
    })
}

async fn call(method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let app = xhub_wallet_v3_6::api::router();
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
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
    (status, serde_json::from_slice(&bytes).expect("json"))
}

#[tokio::test]
async fn prepare_and_confirm_flow_requires_explicit_matching_confirmation() {
    let app = xhub_wallet_v3_6::api::router();
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v3.6/funding-drafts")
                .header("content-type", "application/json")
                .body(Body::from(prepare_body().to_string()))
                .expect("request"),
        )
        .await
        .expect("prepare");
    assert_eq!(response.status(), StatusCode::OK);
    let draft: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(draft["preview"]["close_delay_blocks"], 12_488);
    assert_eq!(
        draft["preview"]["funding_puzzle_hash"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let draft_id = draft["draft_id"].as_str().expect("draft id");
    let terms_hash = draft["preview"]["channel_terms_hash"]
        .as_str()
        .expect("hash");

    let denied = app.clone().oneshot(
        Request::post(format!("/api/v3.6/funding-drafts/{draft_id}/confirm"))
            .header("content-type", "application/json")
            .body(Body::from(json!({"protocol_version":"0x0360","channel_terms_hash":terms_hash,"user_confirmed":false}).to_string()))
            .expect("request"),
    ).await.expect("denied");
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);

    let confirmed = app.clone().oneshot(
        Request::post(format!("/api/v3.6/funding-drafts/{draft_id}/confirm"))
            .header("content-type", "application/json")
            .body(Body::from(json!({"protocol_version":"0x0360","channel_terms_hash":terms_hash,"user_confirmed":true}).to_string()))
            .expect("request"),
    ).await.expect("confirmed");
    assert_eq!(confirmed.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(confirmed.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(body["confirmed"], true);

    let fetched = app
        .oneshot(
            Request::get(format!("/api/v3.6/funding-drafts/{draft_id}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("get");
    assert_eq!(fetched.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejects_wrong_version_and_invalid_terms() {
    let mut wrong = prepare_body();
    wrong["protocol_version"] = json!("0x0350");
    let (status, body) = call("POST", "/api/v3.6/funding-drafts", Some(wrong)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "PROTOCOL_VERSION_MISMATCH");

    let mut invalid = prepare_body();
    invalid["freeze_blocks"] = json!("0");
    let (status, body) = call("POST", "/api/v3.6/funding-drafts", Some(invalid)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_TERMS");
}

#[tokio::test]
async fn exposes_the_mainnet_canary_profile_without_secrets() {
    let (status, body) = call("GET", "/api/v3.6/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["network"], "mainnet");
    assert_eq!(body["network_id"], xhub_wallet_v3_6::MAINNET_NETWORK_ID);
    assert_eq!(body["profile_id"], "v3.6-mainnet-canary-1");
    assert_eq!(body["delivery_threshold"], 2);
    assert_eq!(body["delivery_participants"], 3);
    assert_eq!(body["mainnet_approved"], false);
    assert_eq!(body["production_ready"], false);
    assert_eq!(body["hub_gateway_enabled"], false);
    assert!(body.get("bearer_token").is_none());
    assert!(body.get("hub_base_url").is_none());
}

#[tokio::test]
async fn gateway_adds_the_hub_token_server_side_and_preserves_upstream_status() {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        http::HeaderMap,
        routing::{get, post},
    };

    let seen = Arc::new(Mutex::new(Vec::<(String, String, Option<Value>)>::new()));
    let health_seen = Arc::clone(&seen);
    let registration_seen = Arc::clone(&seen);
    let reservation_seen = Arc::clone(&seen);
    let upstream = Router::new()
        .route(
            "/api/v3.6/health",
            get(move |headers: HeaderMap| {
                let seen = Arc::clone(&health_seen);
                async move {
                    seen.lock().expect("seen").push((
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("x-xhub-protocol-version")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        None,
                    ));
                    Json(json!({"protocol_version":"0x0360","service":"hub","status":"READY"}))
                }
            }),
        )
        .route(
            "/api/v3.6/funding-coins",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = Arc::clone(&registration_seen);
                async move {
                    seen.lock().expect("seen").push((
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("x-xhub-protocol-version")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        Some(body),
                    ));
                    (StatusCode::CREATED, Json(json!({"chain_state":"ACTIVE"})))
                }
            }),
        )
        .route(
            "/api/v3.6/reservations",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = Arc::clone(&reservation_seen);
                async move {
                    seen.lock().expect("seen").push((
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("x-xhub-protocol-version")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        Some(body),
                    ));
                    (StatusCode::ACCEPTED, Json(json!({"status":"PENDING"})))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.expect("mock HUB");
    });

    let token = "gateway-test-token-with-at-least-32-characters";
    let gateway = xhub_wallet_v3_6::api::HubGatewayConfig::new(format!("http://{address}"), token)
        .expect("gateway config");
    let app = xhub_wallet_v3_6::api::router_with_gateway(
        xhub_wallet_v3_6::api::DEFAULT_HUB_STATE_PUBLIC_KEY_A,
        gateway,
    )
    .expect("router");

    let health = app
        .clone()
        .oneshot(
            Request::get("/api/v3.6/hub/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("health");
    assert_eq!(health.status(), StatusCode::OK);
    let health_body = to_bytes(health.into_body(), usize::MAX)
        .await
        .expect("health body");
    assert!(!String::from_utf8_lossy(&health_body).contains(token));

    let registration_body = json!({
        "protocol_version":"0x0360",
        "funding_coin_id":"42".repeat(32),
        "funding_puzzle_reveal_hex":"ff01",
        "channel_terms_canonical_hex":"0360"
    });
    let registration = app
        .clone()
        .oneshot(
            Request::post("/api/v3.6/hub/funding-coins")
                .header("content-type", "application/json")
                .header("authorization", "Bearer browser-must-not-control-this")
                .body(Body::from(registration_body.to_string()))
                .expect("request"),
        )
        .await
        .expect("registration");
    assert_eq!(registration.status(), StatusCode::CREATED);

    let reservation_body = json!({"protocol_version":"0x0360","signed":"opaque"});
    let reservation = app
        .oneshot(
            Request::post("/api/v3.6/hub/reservations")
                .header("content-type", "application/json")
                .header("authorization", "Bearer browser-must-not-control-this")
                .body(Body::from(reservation_body.to_string()))
                .expect("request"),
        )
        .await
        .expect("reservation");
    assert_eq!(reservation.status(), StatusCode::ACCEPTED);

    let seen = seen.lock().expect("seen");
    assert_eq!(seen.len(), 3);
    for (authorization, version, _) in seen.iter() {
        assert_eq!(authorization, &format!("Bearer {token}"));
        assert_eq!(version, "0x0360");
    }
    assert_eq!(seen[1].2.as_ref(), Some(&registration_body));
    assert_eq!(seen[2].2.as_ref(), Some(&reservation_body));
    server.abort();
}

#[test]
fn gateway_rejects_non_loopback_upstreams() {
    let error = xhub_wallet_v3_6::api::HubGatewayConfig::new(
        "https://hub.chiagame.top",
        "gateway-test-token-with-at-least-32-characters",
    )
    .expect_err("public upstream must be rejected");
    assert!(error.contains("loopback"));
}
