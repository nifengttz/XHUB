use std::{fs, path::Path};

use xhub_wallet_v3_6::api::{HubGatewayConfig, router_with_gateway};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("XHUB Wallet V3.6 startup failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let address =
        std::env::var("XHUB_WALLET_LISTEN").unwrap_or_else(|_| "127.0.0.1:8736".to_string());
    if !address.starts_with("127.0.0.1:") && !address.starts_with("[::1]:") {
        return Err("XHUB_WALLET_LISTEN must use a loopback address".into());
    }
    let hub_public_key = required("XHUB_WALLET_HUB_STATE_PUBLIC_KEY_A")?;
    let hub_base_url = required("XHUB_WALLET_HUB_BASE_URL")?;
    let hub_token_path = required("XHUB_WALLET_HUB_API_TOKEN_FILE")?;
    let hub_token = read_secret(&hub_token_path)?;
    let gateway = HubGatewayConfig::new(hub_base_url, hub_token)?;
    let app = router_with_gateway(&hub_public_key, gateway)?;
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|error| format!("cannot bind wallet listener: {error}"))?;
    println!("XHUB Wallet V3.6 mainnet canary listening on http://{address}");
    axum::serve(listener, app)
        .await
        .map_err(|error| format!("wallet server failed: {error}"))
}

fn required(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn read_secret(path: &str) -> Result<String, String> {
    let value = fs::read_to_string(Path::new(path))
        .map_err(|error| format!("cannot read HUB token file: {error}"))?
        .trim()
        .to_string();
    if value.len() < 32 {
        return Err("HUB token file must contain at least 32 characters".into());
    }
    Ok(value)
}
