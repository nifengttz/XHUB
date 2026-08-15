use std::{fs, path::PathBuf, time::Duration};

use reqwest::{Identity, blocking::Client};
use serde_json::{Value, json};
use thiserror::Error;
use xhub_protocol_v3_6::Bytes32;

pub const MAINNET_GENESIS_CHALLENGE_HEX: &str =
    "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainPeak {
    pub height: u64,
    pub header_hash: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FundingCoinState {
    Missing,
    Confirmed {
        birth_height: u64,
        puzzle_hash: Bytes32,
        amount: u64,
    },
    Spent {
        birth_height: u64,
        spent_height: u64,
        puzzle_hash: Bytes32,
        amount: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSnapshot {
    pub network_id: Bytes32,
    pub synced: bool,
    pub peak: Option<ChainPeak>,
    pub funding_coin: FundingCoinState,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ChainProviderError {
    #[error("chain RPC is unavailable: {0}")]
    RpcUnavailable(String),
    #[error("chain RPC returned invalid or incomplete data: {0}")]
    InvalidResponse(String),
    #[error("configured chain sources disagree: {0}")]
    ConflictingSources(String),
}

pub type ChainProviderResult<T> = std::result::Result<T, ChainProviderError>;

pub trait ChainStateProvider: Send + Sync {
    fn snapshot(&self, funding_coin_id: Bytes32) -> ChainProviderResult<ChainSnapshot>;
}

#[derive(Debug, Clone)]
pub struct RedundantChainStateProvider<Primary, Secondary> {
    pub primary: Primary,
    pub secondary: Secondary,
    pub max_peak_height_delta: u64,
}

impl<Primary, Secondary> ChainStateProvider for RedundantChainStateProvider<Primary, Secondary>
where
    Primary: ChainStateProvider,
    Secondary: ChainStateProvider,
{
    fn snapshot(&self, funding_coin_id: Bytes32) -> ChainProviderResult<ChainSnapshot> {
        let primary = self.primary.snapshot(funding_coin_id)?;
        let secondary = self.secondary.snapshot(funding_coin_id)?;
        reconcile_snapshots(primary, secondary, self.max_peak_height_delta)
    }
}

pub fn reconcile_snapshots(
    primary: ChainSnapshot,
    secondary: ChainSnapshot,
    max_peak_height_delta: u64,
) -> ChainProviderResult<ChainSnapshot> {
    if primary.network_id != secondary.network_id {
        return Err(ChainProviderError::ConflictingSources(
            "network_id mismatch".into(),
        ));
    }
    if primary.funding_coin != secondary.funding_coin {
        return Err(ChainProviderError::ConflictingSources(
            "Funding Coin record mismatch".into(),
        ));
    }
    if primary.synced != secondary.synced {
        return Err(ChainProviderError::ConflictingSources(
            "sync status mismatch".into(),
        ));
    }
    let (primary_peak, secondary_peak) = match (&primary.peak, &secondary.peak) {
        (Some(primary_peak), Some(secondary_peak)) => (primary_peak, secondary_peak),
        (None, None) => return Ok(primary),
        _ => {
            return Err(ChainProviderError::ConflictingSources(
                "one source has no peak".into(),
            ));
        }
    };
    let delta = primary_peak.height.abs_diff(secondary_peak.height);
    if delta > max_peak_height_delta {
        return Err(ChainProviderError::ConflictingSources(format!(
            "peak height delta {delta} exceeds {max_peak_height_delta}"
        )));
    }
    if primary_peak.height == secondary_peak.height
        && primary_peak.header_hash != secondary_peak.header_hash
    {
        return Err(ChainProviderError::ConflictingSources(
            "same-height peak header hash mismatch".into(),
        ));
    }
    if secondary_peak.height > primary_peak.height {
        Ok(secondary)
    } else {
        Ok(primary)
    }
}

#[derive(Debug, Clone)]
pub struct ChiaFullNodeRpcConfig {
    pub base_url: String,
    pub client_certificate_pem: Option<PathBuf>,
    pub client_private_key_pem: Option<PathBuf>,
    pub timeout: Duration,
}

impl ChiaFullNodeRpcConfig {
    pub fn public(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client_certificate_pem: None,
            client_private_key_pem: None,
            timeout: Duration::from_secs(15),
        }
    }

    pub fn mutual_tls(
        base_url: impl Into<String>,
        client_certificate_pem: impl Into<PathBuf>,
        client_private_key_pem: impl Into<PathBuf>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            client_certificate_pem: Some(client_certificate_pem.into()),
            client_private_key_pem: Some(client_private_key_pem.into()),
            timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChiaFullNodeRpcProvider {
    client: Client,
    base_url: String,
}

impl ChiaFullNodeRpcProvider {
    pub fn connect(config: ChiaFullNodeRpcConfig) -> ChainProviderResult<Self> {
        let mut builder = Client::builder().timeout(config.timeout);
        match (config.client_certificate_pem, config.client_private_key_pem) {
            (Some(certificate), Some(private_key)) => {
                let mut identity_pem = fs::read(&certificate).map_err(|error| {
                    ChainProviderError::InvalidResponse(format!(
                        "cannot read {}: {error}",
                        certificate.display()
                    ))
                })?;
                identity_pem.push(b'\n');
                identity_pem.extend(fs::read(&private_key).map_err(|error| {
                    ChainProviderError::InvalidResponse(format!(
                        "cannot read {}: {error}",
                        private_key.display()
                    ))
                })?);
                let identity = Identity::from_pem(&identity_pem).map_err(|error| {
                    ChainProviderError::InvalidResponse(format!("invalid RPC identity: {error}"))
                })?;
                builder = builder.identity(identity);
            }
            (None, None) => {}
            _ => {
                return Err(ChainProviderError::InvalidResponse(
                    "both RPC client certificate and private key are required".into(),
                ));
            }
        }
        let client = builder.build().map_err(|error| {
            ChainProviderError::InvalidResponse(format!("cannot build RPC client: {error}"))
        })?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
        })
    }

    fn call(&self, method: &str, body: Value) -> ChainProviderResult<Value> {
        let response = self
            .client
            .post(format!("{}/{method}", self.base_url))
            .json(&body)
            .send()
            .map_err(|error| ChainProviderError::RpcUnavailable(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ChainProviderError::RpcUnavailable(format!(
                "HTTP {status} from {method}"
            )));
        }
        let value = response.json::<Value>().map_err(|error| {
            ChainProviderError::InvalidResponse(format!("invalid {method} JSON: {error}"))
        })?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(ChainProviderError::InvalidResponse(format!(
                "{method} rejected: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            )));
        }
        Ok(value)
    }
}

impl ChainStateProvider for ChiaFullNodeRpcProvider {
    fn snapshot(&self, funding_coin_id: Bytes32) -> ChainProviderResult<ChainSnapshot> {
        let network = self.call("get_network_info", json!({}))?;
        let network_id = parse_network_id(&network)?;

        let blockchain = self.call("get_blockchain_state", json!({}))?;
        let state = blockchain
            .get("blockchain_state")
            .ok_or_else(|| invalid("get_blockchain_state.blockchain_state is missing"))?;
        let synced = state
            .pointer("/sync/synced")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("blockchain_state.sync.synced is missing"))?;
        let peak = match state.get("peak") {
            Some(Value::Null) | None => None,
            Some(peak) => Some(ChainPeak {
                height: parse_u64(
                    peak.get("height")
                        .ok_or_else(|| invalid("peak.height is missing"))?,
                    "peak.height",
                )?,
                header_hash: parse_bytes32(
                    peak.get("header_hash")
                        .ok_or_else(|| invalid("peak.header_hash is missing"))?,
                    "peak.header_hash",
                )?,
            }),
        };

        let coin_response = self.call(
            "get_coin_record_by_name",
            json!({"name": format!("0x{}", hex::encode(funding_coin_id))}),
        )?;
        let funding_coin = match coin_response.get("coin_record") {
            None | Some(Value::Null) => FundingCoinState::Missing,
            Some(record) => parse_coin_record(record)?,
        };
        Ok(ChainSnapshot {
            network_id,
            synced,
            peak,
            funding_coin,
        })
    }
}

fn parse_coin_record(record: &Value) -> ChainProviderResult<FundingCoinState> {
    let coin = record
        .get("coin")
        .ok_or_else(|| invalid("coin_record.coin is missing"))?;
    let birth_height = parse_u64(
        record
            .get("confirmed_block_index")
            .ok_or_else(|| invalid("confirmed_block_index is missing"))?,
        "confirmed_block_index",
    )?;
    let puzzle_hash = parse_bytes32(
        coin.get("puzzle_hash")
            .ok_or_else(|| invalid("coin.puzzle_hash is missing"))?,
        "coin.puzzle_hash",
    )?;
    let amount = parse_u64(
        coin.get("amount")
            .ok_or_else(|| invalid("coin.amount is missing"))?,
        "coin.amount",
    )?;
    let spent = record
        .get("spent")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("coin_record.spent is missing"))?;
    if spent {
        Ok(FundingCoinState::Spent {
            birth_height,
            spent_height: parse_u64(
                record
                    .get("spent_block_index")
                    .ok_or_else(|| invalid("spent_block_index is missing"))?,
                "spent_block_index",
            )?,
            puzzle_hash,
            amount,
        })
    } else {
        Ok(FundingCoinState::Confirmed {
            birth_height,
            puzzle_hash,
            amount,
        })
    }
}

fn parse_network_id(network: &Value) -> ChainProviderResult<Bytes32> {
    if let Some(genesis_challenge) = network.get("genesis_challenge") {
        return parse_bytes32(genesis_challenge, "genesis_challenge");
    }
    match network.get("network_name").and_then(Value::as_str) {
        Some("mainnet") => parse_bytes32(
            &Value::String(MAINNET_GENESIS_CHALLENGE_HEX.into()),
            "known mainnet genesis_challenge",
        ),
        Some(name) => Err(invalid(format!(
            "get_network_info omitted genesis_challenge for unknown network {name}"
        ))),
        None => Err(invalid(
            "get_network_info has neither genesis_challenge nor network_name",
        )),
    }
}

fn parse_u64(value: &Value, field: &str) -> ChainProviderResult<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| invalid(format!("{field} is not a u64")))
}

fn parse_bytes32(value: &Value, field: &str) -> ChainProviderResult<Bytes32> {
    let value = value
        .as_str()
        .ok_or_else(|| invalid(format!("{field} is not a hex string")))?;
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| invalid(format!("invalid {field}: {error}")))?;
    bytes
        .try_into()
        .map_err(|_| invalid(format!("{field} is not 32 bytes")))
}

fn invalid(message: impl Into<String>) -> ChainProviderError {
    ChainProviderError::InvalidResponse(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unspent_and_spent_coin_records() {
        let unspent = json!({
            "confirmed_block_index": 123,
            "spent": false,
            "spent_block_index": 0,
            "coin": {"puzzle_hash": format!("0x{}", "11".repeat(32)), "amount": 500}
        });
        assert_eq!(
            parse_coin_record(&unspent).expect("unspent"),
            FundingCoinState::Confirmed {
                birth_height: 123,
                puzzle_hash: [0x11; 32],
                amount: 500
            }
        );

        let mut spent = unspent;
        spent["spent"] = json!(true);
        spent["spent_block_index"] = json!(456);
        assert!(matches!(
            parse_coin_record(&spent),
            Ok(FundingCoinState::Spent {
                spent_height: 456,
                ..
            })
        ));
    }

    #[test]
    fn accepts_coinset_mainnet_name_but_rejects_unknown_name_without_genesis() {
        assert_eq!(
            hex::encode(parse_network_id(&json!({"network_name":"mainnet"})).expect("mainnet")),
            MAINNET_GENESIS_CHALLENGE_HEX
        );
        assert!(matches!(
            parse_network_id(&json!({"network_name":"unknown-net"})),
            Err(ChainProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_incomplete_mtls_configuration() {
        let config = ChiaFullNodeRpcConfig {
            base_url: "https://127.0.0.1:8555".into(),
            client_certificate_pem: Some("public.crt".into()),
            client_private_key_pem: None,
            timeout: Duration::from_secs(1),
        };
        assert!(matches!(
            ChiaFullNodeRpcProvider::connect(config),
            Err(ChainProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn redundant_sources_choose_the_higher_safe_peak_and_reject_conflicts() {
        let base = ChainSnapshot {
            network_id: [0xaa; 32],
            synced: true,
            peak: Some(ChainPeak {
                height: 100,
                header_hash: [0x10; 32],
            }),
            funding_coin: FundingCoinState::Confirmed {
                birth_height: 50,
                puzzle_hash: [0x20; 32],
                amount: 500,
            },
        };
        let mut ahead = base.clone();
        ahead.peak = Some(ChainPeak {
            height: 101,
            header_hash: [0x11; 32],
        });
        assert_eq!(
            reconcile_snapshots(base.clone(), ahead.clone(), 2)
                .expect("compatible sources")
                .peak,
            ahead.peak
        );

        let mut far_ahead = ahead.clone();
        far_ahead.peak.as_mut().expect("peak").height = 110;
        assert!(matches!(
            reconcile_snapshots(base.clone(), far_ahead, 2),
            Err(ChainProviderError::ConflictingSources(_))
        ));

        let mut fork = base.clone();
        fork.peak.as_mut().expect("peak").header_hash = [0x99; 32];
        assert!(matches!(
            reconcile_snapshots(base, fork, 2),
            Err(ChainProviderError::ConflictingSources(_))
        ));
    }
}
