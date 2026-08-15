#[tokio::main]
async fn main() {
    let address =
        std::env::var("XHUB_WALLET_LISTEN").unwrap_or_else(|_| "127.0.0.1:8736".to_string());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("wallet listener");
    println!("XHUB Wallet V3.6 listening on http://{address}");
    axum::serve(listener, xhub_wallet_v3_6::api::router())
        .await
        .expect("wallet server");
}
