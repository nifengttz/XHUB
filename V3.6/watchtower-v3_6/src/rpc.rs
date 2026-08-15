use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};
use xhub_protocol_v3_6::Bytes32;

use crate::monitor::{ChainPeak, MonitorError, ObservedCoin};

#[derive(Debug, Clone)]
pub struct RpcChainView {
    pub network_id: Bytes32,
    pub synced: bool,
    pub peak: ChainPeak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinSpend {
    pub puzzle_reveal: Vec<u8>,
    pub solution: Vec<u8>,
}

pub trait WatchtowerChainProvider: Send + Sync {
    fn chain_view(&self) -> Result<RpcChainView, MonitorError>;
    fn coin(&self, coin_id: Bytes32) -> Result<Option<ObservedCoin>, MonitorError>;
    fn coin_spend(&self, coin_id: Bytes32, spent_height: u64) -> Result<CoinSpend, MonitorError>;
}

#[derive(Debug, Clone)]
pub struct ChiaRpcProvider {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum ReadOnlyRpcMethod {
    GetNetworkInfo,
    GetBlockchainState,
    GetCoinRecordByName,
    GetPuzzleAndSolution,
}

impl ReadOnlyRpcMethod {
    #[cfg(test)]
    const ALL: [Self; 4] = [
        Self::GetNetworkInfo,
        Self::GetBlockchainState,
        Self::GetCoinRecordByName,
        Self::GetPuzzleAndSolution,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::GetNetworkInfo => "get_network_info",
            Self::GetBlockchainState => "get_blockchain_state",
            Self::GetCoinRecordByName => "get_coin_record_by_name",
            Self::GetPuzzleAndSolution => "get_puzzle_and_solution",
        }
    }
}

impl ChiaRpcProvider {
    pub fn public(base_url: impl Into<String>, timeout: Duration) -> Result<Self, MonitorError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| MonitorError::Unknown(error.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    fn call(&self, method: ReadOnlyRpcMethod, body: Value) -> Result<Value, MonitorError> {
        let method = method.as_str();
        let response = self
            .client
            .post(format!("{}/{method}", self.base_url))
            .json(&body)
            .send()
            .map_err(|error| MonitorError::Unknown(format!("{method}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(MonitorError::Unknown(format!("{method}: HTTP {status}")));
        }
        let value: Value = response
            .json()
            .map_err(|error| MonitorError::Unknown(format!("{method}: invalid JSON: {error}")))?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(MonitorError::Unknown(format!(
                "{method}: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("RPC rejected request")
            )));
        }
        Ok(value)
    }
}

impl WatchtowerChainProvider for ChiaRpcProvider {
    fn chain_view(&self) -> Result<RpcChainView, MonitorError> {
        let network = self.call(ReadOnlyRpcMethod::GetNetworkInfo, json!({}))?;
        let network_id = if let Some(value) = network.get("genesis_challenge") {
            parse_bytes32(value, "genesis_challenge")?
        } else if network.get("network_name").and_then(Value::as_str) == Some("mainnet") {
            hex32("ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb")?
        } else {
            return Err(MonitorError::Unknown(
                "get_network_info omitted a recognized genesis challenge".into(),
            ));
        };
        let response = self.call(ReadOnlyRpcMethod::GetBlockchainState, json!({}))?;
        let state = response.get("blockchain_state").ok_or_else(|| {
            MonitorError::Unknown("get_blockchain_state omitted blockchain_state".into())
        })?;
        let synced = state
            .pointer("/sync/synced")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                MonitorError::Unknown("get_blockchain_state omitted sync.synced".into())
            })?;
        let peak = state
            .get("peak")
            .filter(|value| !value.is_null())
            .ok_or_else(|| MonitorError::Unknown("get_blockchain_state omitted peak".into()))?;
        Ok(RpcChainView {
            network_id,
            synced,
            peak: ChainPeak {
                height: parse_u64(peak.get("height"), "peak.height")?,
                header_hash: parse_bytes32_value(peak.get("header_hash"), "peak.header_hash")?,
            },
        })
    }

    fn coin(&self, coin_id: Bytes32) -> Result<Option<ObservedCoin>, MonitorError> {
        let response = self.call(
            ReadOnlyRpcMethod::GetCoinRecordByName,
            json!({"name": format!("0x{}", hex::encode(coin_id))}),
        )?;
        let Some(record) = response.get("coin_record").filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let coin = record
            .get("coin")
            .ok_or_else(|| MonitorError::Unknown("coin_record omitted coin".into()))?;
        let spent = record
            .get("spent")
            .and_then(Value::as_bool)
            .ok_or_else(|| MonitorError::Unknown("coin_record omitted spent".into()))?;
        Ok(Some(ObservedCoin {
            coin_id,
            parent_coin_id: parse_bytes32_value(coin.get("parent_coin_info"), "parent_coin_info")?,
            puzzle_hash: parse_bytes32_value(coin.get("puzzle_hash"), "puzzle_hash")?,
            amount: parse_u64(coin.get("amount"), "amount")?,
            birth_height: parse_u64(record.get("confirmed_block_index"), "confirmed_block_index")?,
            spent_height: spent
                .then(|| parse_u64(record.get("spent_block_index"), "spent_block_index"))
                .transpose()?,
        }))
    }

    fn coin_spend(&self, coin_id: Bytes32, spent_height: u64) -> Result<CoinSpend, MonitorError> {
        let response = self.call(
            ReadOnlyRpcMethod::GetPuzzleAndSolution,
            json!({
                "coin_id": format!("0x{}", hex::encode(coin_id)),
                "height": spent_height,
            }),
        )?;
        let spend = response
            .get("coin_solution")
            .or_else(|| response.get("coin_spend"))
            .ok_or_else(|| {
                MonitorError::Unknown("get_puzzle_and_solution omitted coin spend".into())
            })?;
        Ok(CoinSpend {
            puzzle_reveal: parse_hex_blob(spend.get("puzzle_reveal"), "puzzle_reveal")?,
            solution: parse_hex_blob(spend.get("solution"), "solution")?,
        })
    }
}

fn parse_u64(value: Option<&Value>, field: &str) -> Result<u64, MonitorError> {
    let value = value.ok_or_else(|| MonitorError::Unknown(format!("RPC omitted {field}")))?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or_else(|| MonitorError::Unknown(format!("RPC {field} is not a canonical u64")))
}

fn parse_bytes32_value(value: Option<&Value>, field: &str) -> Result<Bytes32, MonitorError> {
    parse_bytes32(
        value.ok_or_else(|| MonitorError::Unknown(format!("RPC omitted {field}")))?,
        field,
    )
}

fn parse_bytes32(value: &Value, field: &str) -> Result<Bytes32, MonitorError> {
    let text = value
        .as_str()
        .ok_or_else(|| MonitorError::Unknown(format!("RPC {field} is not hex text")))?;
    hex32(text).map_err(|_| MonitorError::Unknown(format!("RPC {field} is not 32 bytes")))
}

fn hex32(value: &str) -> Result<Bytes32, MonitorError> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| MonitorError::Unknown(error.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| MonitorError::Unknown("expected 32 bytes".into()))
}

fn parse_hex_blob(value: Option<&Value>, field: &str) -> Result<Vec<u8>, MonitorError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| MonitorError::Unknown(format!("RPC omitted {field}")))?;
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| MonitorError::Unknown(format!("RPC {field} is invalid hex: {error}")))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    struct MockResponse {
        path: &'static str,
        status: &'static str,
        body: String,
    }

    fn mock_rpc(responses: Vec<MockResponse>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock RPC listener");
        let address = listener.local_addr().expect("mock RPC address");
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("mock RPC connection");
                let mut request = [0_u8; 8_192];
                let read = stream.read(&mut request).expect("mock RPC request");
                let request = String::from_utf8_lossy(&request[..read]);
                let first_line = request.lines().next().expect("HTTP request line");
                assert_eq!(first_line, format!("POST {} HTTP/1.1", response.path));
                assert!(!first_line.contains("push_tx"));
                let wire = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(wire.as_bytes())
                    .expect("mock RPC response");
            }
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn provider_uses_only_the_read_only_rpc_allowlist() {
        let methods = ReadOnlyRpcMethod::ALL.map(ReadOnlyRpcMethod::as_str);
        assert_eq!(
            methods,
            [
                "get_network_info",
                "get_blockchain_state",
                "get_coin_record_by_name",
                "get_puzzle_and_solution",
            ]
        );
        assert!(!methods.contains(&"push_tx"));
    }

    #[test]
    fn chain_and_coin_contracts_use_only_expected_read_paths() {
        let header_hash = format!("0x{}", hex::encode([0x11; 32]));
        let parent = format!("0x{}", hex::encode([0x22; 32]));
        let puzzle_hash = format!("0x{}", hex::encode([0x33; 32]));
        let blockchain = format!(
            "{{\"success\":true,\"blockchain_state\":{{\"sync\":{{\"synced\":true}},\"peak\":{{\"height\":9148000,\"header_hash\":\"{header_hash}\"}}}}}}"
        );
        let coin = format!(
            "{{\"success\":true,\"coin_record\":{{\"coin\":{{\"parent_coin_info\":\"{parent}\",\"puzzle_hash\":\"{puzzle_hash}\",\"amount\":5}},\"confirmed_block_index\":9146971,\"spent\":false,\"spent_block_index\":0}}}}"
        );
        let responses = vec![
            MockResponse {
                path: "/get_network_info",
                status: "200 OK",
                body: "{\"success\":true,\"network_name\":\"mainnet\"}".into(),
            },
            MockResponse {
                path: "/get_blockchain_state",
                status: "200 OK",
                body: blockchain,
            },
            MockResponse {
                path: "/get_coin_record_by_name",
                status: "200 OK",
                body: coin,
            },
        ];
        let (url, server) = mock_rpc(responses);
        let provider = ChiaRpcProvider::public(url, Duration::from_secs(2)).expect("provider");
        let view = provider.chain_view().expect("chain view");
        assert!(view.synced);
        assert_eq!(view.peak.height, 9_148_000);
        let observed = provider.coin([0x44; 32]).expect("coin RPC").expect("coin");
        assert_eq!(observed.amount, 5);
        assert_eq!(observed.parent_coin_id, [0x22; 32]);
        assert_eq!(observed.puzzle_hash, [0x33; 32]);
        server.join().expect("mock RPC thread");
    }

    #[test]
    fn http_rpc_and_json_failures_are_fail_closed() {
        for response in [
            MockResponse {
                path: "/get_network_info",
                status: "500 Internal Server Error",
                body: "{}".into(),
            },
            MockResponse {
                path: "/get_network_info",
                status: "200 OK",
                body: "{\"success\":false,\"error\":\"injected failure\"}".into(),
            },
            MockResponse {
                path: "/get_network_info",
                status: "200 OK",
                body: "not-json".into(),
            },
        ] {
            let (url, server) = mock_rpc(vec![response]);
            let provider = ChiaRpcProvider::public(url, Duration::from_secs(2)).expect("provider");
            assert!(provider.chain_view().is_err());
            server.join().expect("mock RPC thread");
        }
    }
}
