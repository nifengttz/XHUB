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
        "network_id": "aa".repeat(32),
        "acceptance_blocks": "12288",
        "freeze_blocks": "200",
        "challenge_blocks": "6000",
        "user_public_key": "89d0608036649d3484b7cfe71cfbd7f13015081d6206aede1aed0a4c1ad1521233123c08f0870e9d9f605ed429d24419",
        "hub_state_public_key_a": "b61c4ee5d1cdd57ea615e6f3003e89afeee153d666562d0abec363d8b88c21c35e55f5622668b113e966564d04eb9fa1",
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
