use std::{fs, path::Path};

use serde::Deserialize;
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    api::{ApiState, authenticated_router},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmer {
    signer_id: String,
    failure_domain: String,
    signer_public_key: String,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Watchtower startup failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let listen = optional("XHUB_WATCHTOWER_LISTEN").unwrap_or_else(|| "127.0.0.1:8738".into());
    if !listen.starts_with("127.0.0.1:") && !listen.starts_with("[::1]:") {
        return Err(
            "XHUB_WATCHTOWER_LISTEN must be loopback; expose it through the TLS proxy".into(),
        );
    }
    let mut store = WatchtowerStore::open(required("XHUB_WATCHTOWER_DB")?)
        .map_err(|error| error.to_string())?;
    if let Some(path) = optional("XHUB_WATCHTOWER_CONFIRMERS_FILE") {
        register_confirmers(&mut store, &path)?;
    }
    if let Some(path) = optional("XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE") {
        register_custody_attesters(&mut store, &path)?;
    }
    let token = read_secret(&required("XHUB_WATCHTOWER_API_TOKEN_FILE")?)?;
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|error| error.to_string())?;
    println!("XHUB Watchtower V3.6 listening on http://{listen}");
    axum::serve(listener, authenticated_router(ApiState::new(store), token))
        .await
        .map_err(|error| error.to_string())
}

fn register_confirmers(store: &mut WatchtowerStore, path: &str) -> Result<(), String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read confirmer file {path}: {error}"))?;
    let confirmers: Vec<Confirmer> = serde_json::from_str(&content)
        .map_err(|error| format!("invalid confirmer JSON: {error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    for confirmer in confirmers {
        let public_key = parse_public_key(&confirmer.signer_public_key)?;
        store
            .register_confirmer(
                &confirmer.signer_id,
                &confirmer.failure_domain,
                public_key,
                now,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn register_custody_attesters(store: &mut WatchtowerStore, path: &str) -> Result<(), String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read custody attester file {path}: {error}"))?;
    let attesters: Vec<Confirmer> = serde_json::from_str(&content)
        .map_err(|error| format!("invalid custody attester JSON: {error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    for attester in attesters {
        let public_key = parse_public_key(&attester.signer_public_key)?;
        store
            .register_custody_attester(
                &attester.signer_id,
                &attester.failure_domain,
                public_key,
                now,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn parse_public_key(value: &str) -> Result<[u8; 48], String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    bytes
        .try_into()
        .map_err(|_| "signer_public_key must encode 48 bytes".into())
}

fn read_secret(path: &str) -> Result<String, String> {
    let value = fs::read_to_string(Path::new(path))
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
