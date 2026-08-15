use std::{fs, path::PathBuf, sync::Arc, time::Duration};

use chia_bls::SecretKey;
use serde::Deserialize;
use xhub_hub_v3_6::{
    ChiaFullNodeRpcConfig, ChiaFullNodeRpcProvider, HubStore, WatchtowerHttpTransport,
    WatchtowerHttpTransportSet,
    api::{ApiState, RecoveryPackageTransport, authenticated_router},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("HUB startup failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let listen = optional("XHUB_HUB_LISTEN").unwrap_or_else(|| "127.0.0.1:8737".into());
    if !listen.starts_with("127.0.0.1:") && !listen.starts_with("[::1]:") {
        return Err("XHUB_HUB_LISTEN must be loopback; expose it through the TLS proxy".into());
    }
    let store = HubStore::open(required("XHUB_HUB_DB")?).map_err(|error| error.to_string())?;
    let hub_secret_key = secret_key(&required("XHUB_HUB_BLS_SECRET_FILE")?)?;
    let api_token = secret(&required("XHUB_HUB_API_TOKEN_FILE")?)?;
    let rpc = ChiaFullNodeRpcProvider::connect(rpc_config()?).map_err(|error| error.to_string())?;
    let transport = watchtower_transport()?;
    let state = ApiState::new(store, Arc::new(rpc.clone()), hub_secret_key, transport);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|error| error.to_string())?;
        println!("XHUB HUB V3.6 listening on http://{listen}");
        axum::serve(listener, authenticated_router(state, api_token))
            .await
            .map_err(|error| error.to_string())
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchtowerEndpointConfig {
    recipient_id: String,
    base_url: String,
    api_token_file: String,
}

fn watchtower_transport() -> Result<Arc<dyn RecoveryPackageTransport>, String> {
    if let Some(path) = optional("XHUB_WATCHTOWERS_FILE") {
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read Watchtower config {path}: {error}"))?;
        let endpoints: Vec<WatchtowerEndpointConfig> = serde_json::from_str(&content)
            .map_err(|error| format!("invalid Watchtower config JSON: {error}"))?;
        let transports = endpoints
            .into_iter()
            .map(|endpoint| {
                WatchtowerHttpTransport::new(
                    endpoint.base_url,
                    secret(&endpoint.api_token_file)?,
                    endpoint.recipient_id,
                    Duration::from_secs(15),
                )
                .map_err(|error| error.message)
            })
            .collect::<Result<Vec<_>, String>>()?;
        return WatchtowerHttpTransportSet::new(transports)
            .map(|transport| Arc::new(transport) as Arc<dyn RecoveryPackageTransport>)
            .map_err(|error| error.message);
    }

    let transport = WatchtowerHttpTransport::new(
        required("XHUB_WATCHTOWER_URL")?,
        secret(&required("XHUB_WATCHTOWER_API_TOKEN_FILE")?)?,
        required("XHUB_WATCHTOWER_RECIPIENT_ID")?,
        Duration::from_secs(15),
    )
    .map_err(|error| error.message)?;
    Ok(Arc::new(transport))
}

fn rpc_config() -> Result<ChiaFullNodeRpcConfig, String> {
    let url = required("XHUB_CHIA_RPC_URL")?;
    match (
        optional("XHUB_CHIA_RPC_CERT_FILE"),
        optional("XHUB_CHIA_RPC_KEY_FILE"),
    ) {
        (Some(cert), Some(key)) => Ok(ChiaFullNodeRpcConfig::mutual_tls(url, cert, key)),
        (None, None) => Ok(ChiaFullNodeRpcConfig::public(url)),
        _ => Err("both XHUB_CHIA_RPC_CERT_FILE and XHUB_CHIA_RPC_KEY_FILE are required".into()),
    }
}

fn secret_key(path: &str) -> Result<SecretKey, String> {
    let value = secret(path)?;
    let bytes =
        hex::decode(value).map_err(|error| format!("invalid HUB BLS secret hex: {error}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "HUB BLS secret must encode exactly 32 bytes".to_string())?;
    SecretKey::from_bytes(&bytes).map_err(|error| format!("invalid HUB BLS secret: {error}"))
}

fn secret(path: &str) -> Result<String, String> {
    let value = fs::read_to_string(PathBuf::from(path))
        .map_err(|error| format!("cannot read secret file {path}: {error}"))?
        .trim()
        .to_string();
    if value.len() < 32 {
        return Err(format!(
            "secret file {path} must contain at least 32 characters"
        ));
    }
    Ok(value)
}

fn required(name: &str) -> Result<String, String> {
    optional(name).ok_or_else(|| format!("{name} is required"))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
