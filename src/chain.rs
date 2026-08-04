use std::time::Duration;

use chia_protocol::{Bytes32, SpendBundle};
use chia_sdk_coinset::{
    BlockchainStateResponse, ChiaRpcClient, CoinRecord, CoinsetClient, FullNodeClient,
    GetCoinRecordResponse, GetCoinRecordsResponse, GetMempoolItemResponse,
};
use chia_sdk_types::{MAINNET_CONSTANTS, TESTNET11_CONSTANTS};
use thiserror::Error;

const COST_UNIT: u64 = 5_000_000;

#[derive(Debug, Clone)]
pub enum ChiaRpcConfig {
    PublicMainnet {
        base_url: String,
    },
    PublicTestnet11 {
        base_url: String,
    },
    FullNode {
        base_url: String,
        cert_path: std::path::PathBuf,
        key_path: std::path::PathBuf,
    },
}

impl ChiaRpcConfig {
    pub fn public_mainnet() -> Self {
        Self::PublicMainnet {
            base_url: "https://api.coinset.org".to_string(),
        }
    }

    pub fn public_testnet11() -> Self {
        Self::PublicTestnet11 {
            base_url: "https://testnet11.api.coinset.org".to_string(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ChiaNodeError {
    #[error("RPC transport error: {0}")]
    Transport(String),
    #[error("Chia node rejected request: {0}")]
    Rejected(String),
    #[error("invalid RPC response: {0}")]
    Response(String),
    #[error("node configuration error: {0}")]
    Config(String),
}

#[derive(Debug)]
enum RpcEndpoint {
    Public(CoinsetClient),
    FullNode(FullNodeClient),
}

#[derive(Debug)]
pub struct ChiaNode {
    endpoint: RpcEndpoint,
    expected_genesis_challenge: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStatus {
    pub network_name: Option<String>,
    pub synced: bool,
    pub peak_height: u32,
    pub peak_hash: Bytes32,
    pub mempool_size: u32,
    pub mempool_min_fee_per_cost_unit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MempoolStatus {
    Pending { fee: u64 },
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ChainObservation {
    pub tx_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub peak_height: u32,
    pub peak_hash: Bytes32,
    pub funding_coin: Option<CoinRecord>,
    pub children: Vec<CoinRecord>,
    pub confirmed_height: Option<u32>,
    pub mempool: MempoolStatus,
}

impl ChiaNode {
    pub fn connect(
        config: ChiaRpcConfig,
        expected_genesis_challenge: Bytes32,
    ) -> Result<Self, ChiaNodeError> {
        let endpoint = match config {
            ChiaRpcConfig::PublicMainnet { base_url }
            | ChiaRpcConfig::PublicTestnet11 { base_url } => {
                RpcEndpoint::Public(CoinsetClient::new(base_url))
            }
            ChiaRpcConfig::FullNode {
                base_url,
                cert_path,
                key_path,
            } => {
                let cert = std::fs::read(&cert_path).map_err(|error| {
                    ChiaNodeError::Config(format!("{}: {error}", cert_path.display()))
                })?;
                let key = std::fs::read(&key_path).map_err(|error| {
                    ChiaNodeError::Config(format!("{}: {error}", key_path.display()))
                })?;
                RpcEndpoint::FullNode(
                    FullNodeClient::with_base_url(base_url, &cert, &key)
                        .map_err(|error| ChiaNodeError::Config(error.to_string()))?,
                )
            }
        };
        Ok(Self {
            endpoint,
            expected_genesis_challenge,
        })
    }

    pub fn testnet11() -> Result<Self, ChiaNodeError> {
        Self::connect(
            ChiaRpcConfig::public_testnet11(),
            TESTNET11_CONSTANTS.genesis_challenge,
        )
    }

    pub fn mainnet() -> Result<Self, ChiaNodeError> {
        Self::connect(
            ChiaRpcConfig::public_mainnet(),
            MAINNET_CONSTANTS.genesis_challenge,
        )
    }

    pub async fn status(&self) -> Result<NodeStatus, ChiaNodeError> {
        let (network, genesis_challenge) = self.network_info().await?;
        if let Some(genesis_challenge) = genesis_challenge
            && genesis_challenge != self.expected_genesis_challenge
        {
            return Err(ChiaNodeError::Rejected(format!(
                "wrong genesis challenge: expected {}, got {}",
                self.expected_genesis_challenge, genesis_challenge
            )));
        }
        let response = self.blockchain_state().await?;
        let state = response
            .blockchain_state
            .ok_or_else(|| ChiaNodeError::Response("missing blockchain_state".to_string()))?;
        let peak = serde_json::to_value(&state.peak)
            .map_err(|error| ChiaNodeError::Response(error.to_string()))?;
        let peak_height = json_u32(&peak, "height")?;
        let peak_hash = json_bytes32(&peak, "header_hash")?;
        if !state.genesis_challenge_initialized {
            return Err(ChiaNodeError::Rejected(
                "genesis challenge is not initialized".to_string(),
            ));
        }
        Ok(NodeStatus {
            network_name: network,
            synced: state.sync.synced,
            peak_height,
            peak_hash,
            mempool_size: state.mempool_size,
            mempool_min_fee_per_cost_unit: state.mempool_min_fees.cost_5000000,
        })
    }

    pub async fn get_coin(&self, coin_id: Bytes32) -> Result<Option<CoinRecord>, ChiaNodeError> {
        let response = match &self.endpoint {
            RpcEndpoint::Public(client) => client
                .get_coin_record_by_name(coin_id)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
            RpcEndpoint::FullNode(client) => client
                .get_coin_record_by_name(coin_id)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
        };
        coin_record(response)
    }

    pub async fn get_unspent_coins(
        &self,
        puzzle_hash: Bytes32,
        min_amount: u64,
    ) -> Result<Vec<CoinRecord>, ChiaNodeError> {
        let response = match &self.endpoint {
            RpcEndpoint::Public(client) => client
                // Chia 2.7.x returns null for spent_block_index when spent
                // records are excluded. Request the complete shape and
                // filter locally so the SDK's u32 field can deserialize it.
                .get_coin_records_by_puzzle_hash(puzzle_hash, None, None, Some(true))
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
            RpcEndpoint::FullNode(client) => client
                .get_coin_records_by_puzzle_hash(puzzle_hash, None, None, Some(true))
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
        };
        let records = coin_records(response)?;
        Ok(records
            .into_iter()
            .filter(|record| !record.spent && record.coin.amount >= min_amount)
            .collect())
    }

    pub async fn children(
        &self,
        parent_coin_id: Bytes32,
    ) -> Result<Vec<CoinRecord>, ChiaNodeError> {
        let response = match &self.endpoint {
            RpcEndpoint::Public(client) => client
                .get_coin_records_by_parent_ids(vec![parent_coin_id], None, None, Some(true))
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
            RpcEndpoint::FullNode(client) => client
                .get_coin_records_by_parent_ids(vec![parent_coin_id], None, None, Some(true))
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
        };
        coin_records(response)
    }

    pub async fn broadcast(&self, bundle: SpendBundle) -> Result<Bytes32, ChiaNodeError> {
        let tx_id = bundle.name();
        let response = match &self.endpoint {
            RpcEndpoint::Public(client) => client
                .push_tx(bundle)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
            RpcEndpoint::FullNode(client) => client
                .push_tx(bundle)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
        };
        if response.success && response.status.eq_ignore_ascii_case("SUCCESS") {
            Ok(tx_id)
        } else {
            Err(ChiaNodeError::Rejected(
                response
                    .error
                    .unwrap_or_else(|| response.status.to_string()),
            ))
        }
    }

    pub async fn broadcast_with_retry(
        &self,
        bundle: SpendBundle,
        max_attempts: usize,
        retry_delay: Duration,
    ) -> Result<Bytes32, ChiaNodeError> {
        let attempts = max_attempts.max(1);
        let mut last_transport_error = None;
        for attempt in 0..attempts {
            match self.broadcast(bundle.clone()).await {
                Ok(tx_id) => return Ok(tx_id),
                Err(error @ ChiaNodeError::Transport(_)) => {
                    last_transport_error = Some(error);
                    if attempt + 1 < attempts {
                        tokio::time::sleep(retry_delay).await;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_transport_error.expect("at least one broadcast attempt"))
    }

    pub async fn mempool_status(&self, tx_id: Bytes32) -> Result<MempoolStatus, ChiaNodeError> {
        let response = match &self.endpoint {
            RpcEndpoint::Public(client) => client
                .get_mempool_item_by_tx_id(tx_id)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
            RpcEndpoint::FullNode(client) => client
                .get_mempool_item_by_tx_id(tx_id)
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string()))?,
        };
        mempool_item(response)
    }

    pub async fn observe(
        &self,
        tx_id: Bytes32,
        funding_coin_id: Bytes32,
    ) -> Result<ChainObservation, ChiaNodeError> {
        let status = self.status().await?;
        let funding_coin = self.get_coin(funding_coin_id).await?;
        let children = self.children(funding_coin_id).await?;
        let confirmed_height = children
            .iter()
            .map(|record| record.confirmed_block_index)
            .filter(|height| *height > 0)
            .min();
        let mempool = if confirmed_height.is_some() {
            MempoolStatus::NotFound
        } else {
            self.mempool_status(tx_id).await?
        };
        Ok(ChainObservation {
            tx_id,
            funding_coin_id,
            peak_height: status.peak_height,
            peak_hash: status.peak_hash,
            funding_coin,
            children,
            confirmed_height,
            mempool,
        })
    }

    pub async fn observe_funding(
        &self,
        funding_coin_id: Bytes32,
    ) -> Result<ChainObservation, ChiaNodeError> {
        let status = self.status().await?;
        let funding_coin = self.get_coin(funding_coin_id).await?;
        let children = self.children(funding_coin_id).await?;
        let confirmed_height = children
            .iter()
            .map(|record| record.confirmed_block_index)
            .filter(|height| *height > 0)
            .min();
        Ok(ChainObservation {
            tx_id: Bytes32::default(),
            funding_coin_id,
            peak_height: status.peak_height,
            peak_hash: status.peak_hash,
            funding_coin,
            children,
            confirmed_height,
            mempool: MempoolStatus::NotFound,
        })
    }

    pub async fn wait_for_confirmation(
        &self,
        tx_id: Bytes32,
        funding_coin_id: Bytes32,
        confirmation_depth: u32,
        poll_interval: Duration,
        max_polls: usize,
    ) -> Result<ChainObservation, ChiaNodeError> {
        let mut last = None;
        for _ in 0..max_polls {
            let observation = self.observe(tx_id, funding_coin_id).await?;
            let confirmed = observation.confirmed_height.is_some_and(|height| {
                observation
                    .peak_height
                    .saturating_sub(height)
                    .saturating_add(1)
                    >= confirmation_depth
            });
            last = Some(observation.clone());
            if confirmed {
                return Ok(observation);
            }
            tokio::time::sleep(poll_interval).await;
        }
        Err(ChiaNodeError::Response(format!(
            "confirmation timeout after {max_polls} polls; last={last:?}"
        )))
    }

    pub async fn estimate_fee(&self, cost: u64, margin_bps: u64) -> Result<u64, ChiaNodeError> {
        let status = self.status().await?;
        let units = cost.saturating_add(COST_UNIT - 1) / COST_UNIT;
        let base = units
            .checked_mul(status.mempool_min_fee_per_cost_unit)
            .ok_or_else(|| ChiaNodeError::Response("fee overflow".to_string()))?;
        base.checked_mul(10_000 + margin_bps)
            .and_then(|value| value.checked_add(9_999))
            .map(|value| value / 10_000)
            .ok_or_else(|| ChiaNodeError::Response("fee overflow".to_string()))
    }

    pub fn expected_genesis_challenge(&self) -> Bytes32 {
        self.expected_genesis_challenge
    }
}

pub fn select_fee_coin(
    records: impl IntoIterator<Item = CoinRecord>,
    funding_coin_id: Bytes32,
    fee: u64,
) -> Option<CoinRecord> {
    records
        .into_iter()
        .filter(|record| {
            !record.spent && record.coin.coin_id() != funding_coin_id && record.coin.amount >= fee
        })
        .min_by_key(|record| (record.coin.amount, record.coin.coin_id()))
}

pub fn is_reorged(previous: &ChainObservation, current: &ChainObservation) -> bool {
    previous.confirmed_height.is_some()
        && (current.confirmed_height.is_none()
            || current.children.is_empty()
            || current.peak_height < previous.peak_height
            || previous.children.iter().any(|previous_child| {
                !current.children.iter().any(|current_child| {
                    current_child.coin.coin_id() == previous_child.coin.coin_id()
                })
            }))
}

fn coin_record(response: GetCoinRecordResponse) -> Result<Option<CoinRecord>, ChiaNodeError> {
    if !response.success {
        return Err(ChiaNodeError::Rejected(
            response
                .error
                .unwrap_or_else(|| "get_coin_record_by_name failed".to_string()),
        ));
    }
    Ok(response.coin_record)
}

fn coin_records(response: GetCoinRecordsResponse) -> Result<Vec<CoinRecord>, ChiaNodeError> {
    if !response.success {
        return Err(ChiaNodeError::Rejected(
            response
                .error
                .unwrap_or_else(|| "coin query failed".to_string()),
        ));
    }
    Ok(response.coin_records.unwrap_or_default())
}

fn mempool_item(response: GetMempoolItemResponse) -> Result<MempoolStatus, ChiaNodeError> {
    if response.success {
        return Ok(response
            .mempool_item
            .map_or(MempoolStatus::NotFound, |item| MempoolStatus::Pending {
                fee: item.fee,
            }));
    }
    Ok(MempoolStatus::NotFound)
}

async fn network_info<E>(client: &E) -> Result<(Option<String>, Option<Bytes32>), ChiaNodeError>
where
    E: ChiaRpcClient,
    E::Error: std::fmt::Display,
{
    let response = client
        .get_network_info()
        .await
        .map_err(|error| ChiaNodeError::Transport(error_to_string(error)))?;
    if !response.success {
        return Err(ChiaNodeError::Rejected(
            response
                .error
                .unwrap_or_else(|| "get_network_info failed".to_string()),
        ));
    }
    Ok((response.network_name, response.genesis_challenge))
}

fn error_to_string<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

impl ChiaNode {
    async fn network_info(&self) -> Result<(Option<String>, Option<Bytes32>), ChiaNodeError> {
        match &self.endpoint {
            RpcEndpoint::Public(client) => network_info(client).await,
            RpcEndpoint::FullNode(client) => network_info(client).await,
        }
    }

    async fn blockchain_state(&self) -> Result<BlockchainStateResponse, ChiaNodeError> {
        match &self.endpoint {
            RpcEndpoint::Public(client) => client
                .get_blockchain_state()
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string())),
            RpcEndpoint::FullNode(client) => client
                .get_blockchain_state()
                .await
                .map_err(|error| ChiaNodeError::Transport(error.to_string())),
        }
    }
}

fn json_u32(value: &serde_json::Value, field: &str) -> Result<u32, ChiaNodeError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| ChiaNodeError::Response(format!("missing or invalid {field}")))
}

fn json_bytes32(value: &serde_json::Value, field: &str) -> Result<Bytes32, ChiaNodeError> {
    let raw = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ChiaNodeError::Response(format!("missing or invalid {field}")))?;
    let raw = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes = hex::decode(raw)
        .map_err(|error| ChiaNodeError::Response(format!("invalid {field}: {error}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ChiaNodeError::Response(format!("invalid {field} length")))?;
    Ok(Bytes32::from(bytes))
}

#[cfg(test)]
mod tests {
    use chia_protocol::Coin;

    use super::*;

    fn record(amount: u64, id: u8) -> CoinRecord {
        CoinRecord {
            coin: Coin::new(Bytes32::from([id; 32]), Bytes32::from([2; 32]), amount),
            coinbase: false,
            confirmed_block_index: 1,
            spent: false,
            spent_block_index: 0,
            timestamp: 0,
        }
    }

    #[test]
    fn selects_smallest_independent_sufficient_fee_coin() {
        let records = vec![record(10, 1), record(100, 2), record(40, 3)];
        let selected = select_fee_coin(records, Bytes32::from([9; 32]), 35).unwrap();
        assert_eq!(selected.coin.amount, 40);
    }

    #[test]
    fn detects_confirmed_children_going_missing() {
        let mut previous = ChainObservation {
            tx_id: Bytes32::from([1; 32]),
            funding_coin_id: Bytes32::from([2; 32]),
            peak_height: 10,
            peak_hash: Bytes32::from([3; 32]),
            funding_coin: None,
            children: vec![record(1, 4)],
            confirmed_height: Some(9),
            mempool: MempoolStatus::NotFound,
        };
        let current = ChainObservation {
            children: Vec::new(),
            confirmed_height: None,
            peak_height: 10,
            ..previous.clone()
        };
        assert!(is_reorged(&previous, &current));
        previous.peak_height = 11;
        assert!(!is_reorged(&previous, &previous));
    }
}
