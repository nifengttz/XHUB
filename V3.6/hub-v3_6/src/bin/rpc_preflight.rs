use std::process::ExitCode;

use serde_json::json;
use xhub_hub_v3_6::{
    ChainStateProvider, ChiaFullNodeRpcConfig, ChiaFullNodeRpcProvider, FundingCoinState,
};

fn main() -> ExitCode {
    match run() {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).expect("JSON"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("RPC preflight failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<serde_json::Value, String> {
    let url = required("XHUB_CHIA_RPC_URL")?;
    let config = match (
        optional("XHUB_CHIA_RPC_CERT_FILE"),
        optional("XHUB_CHIA_RPC_KEY_FILE"),
    ) {
        (Some(cert), Some(key)) => ChiaFullNodeRpcConfig::mutual_tls(url, cert, key),
        (None, None) => ChiaFullNodeRpcConfig::public(url),
        _ => return Err("both RPC certificate and key files are required".into()),
    };
    let coin_id = parse_coin_id(&required("XHUB_PREFLIGHT_FUNDING_COIN_ID")?)?;
    let snapshot = ChiaFullNodeRpcProvider::connect(config)
        .map_err(|error| error.to_string())?
        .snapshot(coin_id)
        .map_err(|error| error.to_string())?;
    let expected_network = optional("XHUB_EXPECTED_NETWORK_ID")
        .map(|value| parse_coin_id(&value))
        .transpose()?;
    if expected_network.is_some_and(|expected| expected != snapshot.network_id) {
        return Err(
            "connected Full Node genesis challenge does not match XHUB_EXPECTED_NETWORK_ID".into(),
        );
    }
    let peak_height = snapshot.peak.as_ref().map(|peak| peak.height);
    let (funding_coin, funding_ready) = match snapshot.funding_coin {
        FundingCoinState::Missing => (json!({"status":"MISSING"}), false),
        FundingCoinState::Confirmed {
            birth_height,
            puzzle_hash,
            amount,
        } => {
            let confirmations =
                peak_height.map(|peak| peak.saturating_sub(birth_height).saturating_add(1));
            (
                json!({
                    "status":"CONFIRMED", "birth_height":birth_height,
                    "confirmations":confirmations,
                    "puzzle_hash":hex::encode(puzzle_hash), "amount":amount
                }),
                confirmations.is_some_and(|depth| depth >= 32),
            )
        }
        FundingCoinState::Spent {
            birth_height,
            spent_height,
            puzzle_hash,
            amount,
        } => (
            json!({
                "status":"SPENT", "birth_height":birth_height, "spent_height":spent_height,
                "puzzle_hash":hex::encode(puzzle_hash), "amount":amount
            }),
            false,
        ),
    };
    Ok(json!({
        "schema":"xhub-v3-6-rpc-preflight-1", "protocol_version":"0x0360",
        "network_id":hex::encode(snapshot.network_id), "synced":snapshot.synced,
        "peak_height":peak_height, "funding_coin":funding_coin,
        "required_funding_confirmations":32,
        "ready":snapshot.synced && peak_height.is_some() && funding_ready
    }))
}

fn parse_coin_id(value: &str) -> Result<[u8; 32], String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    bytes
        .try_into()
        .map_err(|_| "value must encode 32 bytes".into())
}

fn required(name: &str) -> Result<String, String> {
    optional(name).ok_or_else(|| format!("{name} is required"))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
