use std::time::Duration;

use xhub_hub_v3_6::{
    WatchtowerHttpTransport, WatchtowerHttpTransportSet, api::RecoveryPackageTransport,
};
use xhub_protocol_v3_6::{CanonicalDecode, RecoveryPackage};
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    api::{ApiState, authenticated_router},
};

const TOKEN: &str = "watchtower-integration-token-000000000001";

fn vector_package() -> RecoveryPackage {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../protocol-v3_6/test-vectors/protocol-v3_6.json"
    ))
    .expect("vectors");
    let hex = vectors["recovery_package"]["canonical_hex"]
        .as_str()
        .expect("package hex");
    RecoveryPackage::from_canonical_bytes(&hex::decode(hex).expect("decode hex"))
        .expect("decode package")
}

async fn server(token: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let app = authenticated_router(
        ApiState::new(WatchtowerStore::open_in_memory().expect("store")),
        token.to_string(),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    (format!("http://{address}"), task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivers_vector_package_over_authenticated_http() {
    let (url, task) = server(TOKEN).await;
    let package = vector_package();
    let result = tokio::task::spawn_blocking(move || {
        let transport =
            WatchtowerHttpTransport::new(url, TOKEN, "watchtower-test-1", Duration::from_secs(5))
                .expect("transport");
        transport.deliver(
            "watchtower-test-1",
            "WATCHTOWER",
            "package-vector-1",
            &package,
        )
    })
    .await
    .expect("transport task");
    assert_eq!(result, Ok(()));
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_delivery_is_final_and_wrong_recipient_never_sends() {
    let (url, task) = server(TOKEN).await;
    let package = vector_package();
    let unauthorized_url = url.clone();
    let error = tokio::task::spawn_blocking(move || {
        let wrong_token = WatchtowerHttpTransport::new(
            unauthorized_url,
            "wrong-watchtower-token-000000000000001",
            "watchtower-test-1",
            Duration::from_secs(5),
        )
        .expect("transport");
        wrong_token
            .deliver(
                "watchtower-test-1",
                "WATCHTOWER",
                "package-vector-2",
                &package,
            )
            .expect_err("unauthorized")
    })
    .await
    .expect("transport task");
    assert!(!error.retryable);
    assert!(error.message.contains("401"));

    let error = tokio::task::spawn_blocking(move || {
        let transport =
            WatchtowerHttpTransport::new(url, TOKEN, "watchtower-test-1", Duration::from_secs(5))
                .expect("transport");
        transport
            .deliver(
                "other-watchtower",
                "WATCHTOWER",
                "package-vector-3",
                &vector_package(),
            )
            .expect_err("recipient mismatch")
    })
    .await
    .expect("transport task");
    assert!(!error.retryable);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_set_routes_three_distinct_watchtowers() {
    let (url_a, task_a) = server("watchtower-token-a-00000000000000000001").await;
    let (url_b, task_b) = server("watchtower-token-b-00000000000000000001").await;
    let (url_c, task_c) = server("watchtower-token-c-00000000000000000001").await;
    tokio::task::spawn_blocking(move || {
        let transport = WatchtowerHttpTransportSet::new(vec![
            WatchtowerHttpTransport::new(
                url_a,
                "watchtower-token-a-00000000000000000001",
                "wt-a",
                Duration::from_secs(5),
            )
            .expect("wt-a transport"),
            WatchtowerHttpTransport::new(
                url_b,
                "watchtower-token-b-00000000000000000001",
                "wt-b",
                Duration::from_secs(5),
            )
            .expect("wt-b transport"),
            WatchtowerHttpTransport::new(
                url_c,
                "watchtower-token-c-00000000000000000001",
                "wt-c",
                Duration::from_secs(5),
            )
            .expect("wt-c transport"),
        ])
        .expect("transport set");

        assert_eq!(transport.recipient_ids(), ["wt-a", "wt-b", "wt-c"]);
        for recipient in transport.recipient_ids() {
            transport
                .deliver(
                    &recipient,
                    "WATCHTOWER",
                    &format!("package-{recipient}"),
                    &vector_package(),
                )
                .expect("routed delivery");
        }
        let unknown = transport
            .deliver(
                "wt-unknown",
                "WATCHTOWER",
                "package-unknown",
                &vector_package(),
            )
            .expect_err("unknown recipient");
        assert!(!unknown.retryable);
    })
    .await
    .expect("transport worker");

    task_a.abort();
    task_b.abort();
    task_c.abort();
}
