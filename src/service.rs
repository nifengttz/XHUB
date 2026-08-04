use std::{env, fmt, fs, path::Path, time::Duration};

use chia_bls::{PublicKey, SecretKey};
use chia_protocol::{Bytes32, Coin, SpendBundle};
use chia_sdk_types::{MAINNET_CONSTANTS, TESTNET11_CONSTANTS};
use chia_traits::Streamable;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BroadcastKind, BroadcastRequest, BroadcastState, ChannelArgs, ChannelState, ChannelStore,
    ChiaNode, ChiaNodeError, MerchantInvoice, PaymentIntent, PaymentVoucher,
    SettlementWorkflowError, StateStoreError, build_claim_bundle, build_refund_bundle,
    confirm_claim, confirm_refund,
};

pub const API_SCHEMA_VERSION: u16 = 1;
pub const API_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEnvelope {
    pub schema_version: u16,
    pub protocol_version: u16,
    pub artifact_type: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub channel_id: String,
    pub payload_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artifact {
    Invoice(Box<MerchantInvoice>),
    Intent(Box<PaymentIntent>),
    Voucher(Box<PaymentVoucher>),
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported API schema version")]
    UnsupportedSchema,
    #[error("unsupported artifact type")]
    UnsupportedArtifact,
    #[error("missing idempotency key")]
    MissingIdempotencyKey,
    #[error("invalid artifact payload: {0}")]
    InvalidPayload(String),
    #[error("invalid channel id")]
    InvalidChannelId,
}

impl ApiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson(_) => "INVALID_JSON",
            Self::UnsupportedSchema => "UNSUPPORTED_SCHEMA",
            Self::UnsupportedArtifact => "UNSUPPORTED_ARTIFACT",
            Self::MissingIdempotencyKey => "MISSING_IDEMPOTENCY_KEY",
            Self::InvalidPayload(_) => "INVALID_PAYLOAD",
            Self::InvalidChannelId => "INVALID_CHANNEL_ID",
        }
    }
}

pub fn encode_artifact(
    artifact_type: &str,
    payload: &[u8],
    request_id: impl Into<String>,
    idempotency_key: impl Into<String>,
    channel_id: Bytes32,
) -> Result<String, ApiError> {
    let artifact = decode_payload(artifact_type, payload)?;
    let expected_channel = artifact_channel_id(&artifact);
    if expected_channel != channel_id {
        return Err(ApiError::InvalidChannelId);
    }
    let payload = match artifact {
        Artifact::Invoice(value) => value.to_bytes(),
        Artifact::Intent(value) => value.to_bytes(),
        Artifact::Voucher(value) => value.to_bytes(),
    };
    let idempotency_key = idempotency_key.into();
    if idempotency_key.trim().is_empty() {
        return Err(ApiError::MissingIdempotencyKey);
    }
    let envelope = ArtifactEnvelope {
        schema_version: API_SCHEMA_VERSION,
        protocol_version: API_PROTOCOL_VERSION,
        artifact_type: artifact_type.to_string(),
        request_id: request_id.into(),
        idempotency_key,
        channel_id: format!("0x{}", hex::encode(channel_id)),
        payload_hex: hex::encode(payload),
    };
    serde_json::to_string_pretty(&envelope)
        .map_err(|error| ApiError::InvalidJson(error.to_string()))
}

pub fn decode_artifact(json: &str) -> Result<(ArtifactEnvelope, Artifact), ApiError> {
    let envelope: ArtifactEnvelope =
        serde_json::from_str(json).map_err(|error| ApiError::InvalidJson(error.to_string()))?;
    if envelope.schema_version != API_SCHEMA_VERSION
        || envelope.protocol_version != API_PROTOCOL_VERSION
    {
        return Err(ApiError::UnsupportedSchema);
    }
    if envelope.idempotency_key.trim().is_empty() {
        return Err(ApiError::MissingIdempotencyKey);
    }
    parse_bytes32(&envelope.channel_id).map_err(|_| ApiError::InvalidChannelId)?;
    let payload = hex::decode(&envelope.payload_hex)
        .map_err(|error| ApiError::InvalidPayload(error.to_string()))?;
    let artifact = decode_payload(&envelope.artifact_type, &payload)?;
    if artifact_channel_id(&artifact)
        != parse_bytes32(&envelope.channel_id).map_err(|_| ApiError::InvalidChannelId)?
    {
        return Err(ApiError::InvalidChannelId);
    }
    Ok((envelope, artifact))
}

fn artifact_channel_id(artifact: &Artifact) -> Bytes32 {
    match artifact {
        Artifact::Invoice(value) => value.fields.channel_id,
        Artifact::Intent(value) => value.commitment.channel_id,
        Artifact::Voucher(value) => value.intent.commitment.channel_id,
    }
}

fn decode_payload(artifact_type: &str, payload: &[u8]) -> Result<Artifact, ApiError> {
    match artifact_type {
        "Invoice" => MerchantInvoice::from_bytes(payload)
            .map(|value| Artifact::Invoice(Box::new(value)))
            .map_err(|error| ApiError::InvalidPayload(error.to_string())),
        "Intent" => PaymentIntent::from_bytes(payload)
            .map(|value| Artifact::Intent(Box::new(value)))
            .map_err(|error| ApiError::InvalidPayload(error.to_string())),
        "Voucher" => PaymentVoucher::from_bytes(payload)
            .map(|value| Artifact::Voucher(Box::new(value)))
            .map_err(|error| ApiError::InvalidPayload(error.to_string())),
        _ => Err(ApiError::UnsupportedArtifact),
    }
}

fn parse_bytes32(value: &str) -> Result<Bytes32, ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|_| ApiError::InvalidChannelId)?;
    if bytes.len() != 32 {
        return Err(ApiError::InvalidChannelId);
    }
    Ok(Bytes32::from(<[u8; 32]>::try_from(bytes).unwrap()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WatchAction {
    Idle,
    BroadcastPrepared,
    BroadcastSubmitted,
    Confirmed,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchOutcome {
    pub action: WatchAction,
    pub channel_id: String,
    pub spend_bundle_id: Option<String>,
    pub peak_height: u32,
}

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error(transparent)]
    Store(#[from] StateStoreError),
    #[error(transparent)]
    Node(#[from] ChiaNodeError),
    #[error(transparent)]
    Settlement(#[from] SettlementWorkflowError),
    #[error("spend bundle decode failed: {0}")]
    BundleDecode(String),
    #[error("funding coin is unavailable")]
    FundingCoinUnavailable,
    #[error("funding coin was spent to an unexpected output")]
    UnexpectedSpend,
    #[error("channel height has invalid binary encoding")]
    InvalidHeight,
}

pub async fn merchant_watch_once(
    store: &mut ChannelStore,
    node: &ChiaNode,
    channel_id: Bytes32,
    args: &ChannelArgs,
    _safety_margin: u32,
    confirmation_depth: u32,
) -> Result<WatchOutcome, WatcherError> {
    let record = store.load_channel(channel_id)?;
    let voucher = match record.voucher.clone() {
        Some(voucher) => voucher,
        None => return Ok(outcome(WatchAction::Idle, channel_id, None, 0)),
    };
    let funding_coin_id = voucher.intent.commitment.funding_coin_id;
    let observation = node.observe_funding(funding_coin_id).await?;
    let peak_height = observation.peak_height;
    store.record_chain_observation_with_reorg(channel_id, &observation, None)?;
    let children = observation
        .children
        .iter()
        .map(|record| record.coin)
        .collect::<Vec<Coin>>();
    if observation
        .confirmed_height
        .is_some_and(|height| has_confirmation_depth(peak_height, height, confirmation_depth))
    {
        if children.len() == 2 {
            if store.load_channel(channel_id)?.state == ChannelState::VoucherIssued {
                store.mark_claim_submitted(channel_id)?;
            }
            if store.load_channel(channel_id)?.state == ChannelState::ClaimSubmitted {
                confirm_claim(store, channel_id, funding_coin_id, &children)?;
            }
            let key = claim_key(channel_id);
            if let Some(job) = store.load_broadcast(&key)? {
                store.update_broadcast_state(&key, BroadcastState::Confirmed, None)?;
                return Ok(outcome(
                    WatchAction::Confirmed,
                    channel_id,
                    Some(job.spend_bundle_id),
                    peak_height,
                ));
            }
            return Ok(outcome(
                WatchAction::Confirmed,
                channel_id,
                None,
                peak_height,
            ));
        }
        if children.len() == 1 && children[0].puzzle_hash == args.user_puzzle_hash {
            if store.load_channel(channel_id)?.state == ChannelState::ClaimSubmitted {
                store.mark_refundable(channel_id)?;
            }
            if store.load_channel(channel_id)?.state == ChannelState::Refundable {
                store.mark_refund_submitted(channel_id)?;
            }
            if store.load_channel(channel_id)?.state == ChannelState::RefundSubmitted {
                confirm_refund(
                    store,
                    channel_id,
                    funding_coin_id,
                    args.user_puzzle_hash,
                    &children,
                )?;
            }
            let key = refund_key(channel_id);
            if let Some(job) = store.load_broadcast(&key)? {
                store.update_broadcast_state(&key, BroadcastState::Confirmed, None)?;
                return Ok(outcome(
                    WatchAction::Confirmed,
                    channel_id,
                    Some(job.spend_bundle_id),
                    peak_height,
                ));
            }
            return Ok(outcome(
                WatchAction::Confirmed,
                channel_id,
                None,
                peak_height,
            ));
        }
        return Err(WatcherError::UnexpectedSpend);
    }
    if u64::from(peak_height) > voucher.intent.commitment.claim_before_height {
        if observation
            .funding_coin
            .as_ref()
            .is_some_and(|coin| !coin.spent)
        {
            if matches!(
                record.state,
                ChannelState::Funded
                    | ChannelState::IntentSigned
                    | ChannelState::VoucherIssued
                    | ChannelState::ClaimSubmitted
            ) {
                store.mark_refundable(channel_id)?;
            }
            return Ok(outcome(WatchAction::Expired, channel_id, None, peak_height));
        }
        return Err(WatcherError::UnexpectedSpend);
    }
    let key = claim_key(channel_id);
    let job = if let Some(job) = store.load_broadcast(&key)? {
        job
    } else {
        let bundle = build_claim_bundle(
            observation
                .funding_coin
                .as_ref()
                .ok_or(WatcherError::FundingCoinUnavailable)?
                .coin,
            args,
            &voucher,
        )?;
        store.prepare_broadcast(&BroadcastRequest {
            idempotency_key: &key,
            channel_id,
            kind: BroadcastKind::Claim,
            bundle: &bundle,
            funding_coin_id,
            fee: None,
            fee_coin_id: None,
        })?
    };
    let tracked_observation = node.observe(job.spend_bundle_id, funding_coin_id).await?;
    store.record_chain_observation_with_reorg(channel_id, &tracked_observation, job.fee)?;
    let bundle = SpendBundle::from_bytes(&job.spend_bundle)
        .map_err(|error| WatcherError::BundleDecode(error.to_string()))?;
    if let BroadcastState::Submitted | BroadcastState::Pending = job.state
        && matches!(
            node.mempool_status(job.spend_bundle_id).await?,
            crate::MempoolStatus::Pending { .. }
        )
    {
        return Ok(outcome(
            WatchAction::Idle,
            channel_id,
            Some(job.spend_bundle_id),
            peak_height,
        ));
    }
    store.record_broadcast_attempt(&key, BroadcastState::Pending, None)?;
    match node.broadcast(bundle).await {
        Ok(tx_id) => {
            store.update_broadcast_state(&key, BroadcastState::Submitted, None)?;
            if store.load_channel(channel_id)?.state == ChannelState::VoucherIssued {
                store.mark_claim_submitted(channel_id)?;
            }
            Ok(outcome(
                WatchAction::BroadcastSubmitted,
                channel_id,
                Some(tx_id),
                peak_height,
            ))
        }
        Err(error) => {
            store.update_broadcast_state(
                &key,
                BroadcastState::Rejected,
                Some(&error.to_string()),
            )?;
            Err(error.into())
        }
    }
}

pub async fn user_refund_watch_once(
    store: &mut ChannelStore,
    node: &ChiaNode,
    channel_id: Bytes32,
    args: &ChannelArgs,
    user_secret_key: &chia_bls::SecretKey,
    agg_sig_me_additional_data: Bytes32,
    confirmation_depth: u32,
) -> Result<WatchOutcome, WatcherError> {
    let record = store.load_channel(channel_id)?;
    let funding_coin_id = record
        .voucher
        .as_ref()
        .map(|voucher| voucher.intent.commitment.funding_coin_id)
        .or(record
            .intent
            .as_ref()
            .map(|intent| intent.commitment.funding_coin_id))
        .ok_or(WatcherError::FundingCoinUnavailable)?;
    let observation = node.observe_funding(funding_coin_id).await?;
    store.record_chain_observation_with_reorg(channel_id, &observation, None)?;
    if observation.confirmed_height.is_some_and(|height| {
        has_confirmation_depth(observation.peak_height, height, confirmation_depth)
    }) {
        let children = observation
            .children
            .iter()
            .map(|record| record.coin)
            .collect::<Vec<Coin>>();
        if children.len() == 2 {
            if store.load_channel(channel_id)?.state == ChannelState::VoucherIssued {
                store.mark_claim_submitted(channel_id)?;
            }
            if store.load_channel(channel_id)?.state == ChannelState::ClaimSubmitted {
                confirm_claim(store, channel_id, funding_coin_id, &children)?;
            }
            let key = claim_key(channel_id);
            if let Some(job) = store.load_broadcast(&key)? {
                store.update_broadcast_state(&key, BroadcastState::Confirmed, None)?;
                return Ok(outcome(
                    WatchAction::Confirmed,
                    channel_id,
                    Some(job.spend_bundle_id),
                    observation.peak_height,
                ));
            }
            return Ok(outcome(
                WatchAction::Confirmed,
                channel_id,
                None,
                observation.peak_height,
            ));
        }
        if children.len() == 1 && children[0].puzzle_hash == args.user_puzzle_hash {
            if store.load_channel(channel_id)?.state == ChannelState::Refundable {
                store.mark_refund_submitted(channel_id)?;
            }
            if record.state == ChannelState::RefundSubmitted {
                confirm_refund(
                    store,
                    channel_id,
                    funding_coin_id,
                    args.user_puzzle_hash,
                    &children,
                )?;
            }
            let key = refund_key(channel_id);
            if let Some(job) = store.load_broadcast(&key)? {
                store.update_broadcast_state(&key, BroadcastState::Confirmed, None)?;
                return Ok(outcome(
                    WatchAction::Confirmed,
                    channel_id,
                    Some(job.spend_bundle_id),
                    observation.peak_height,
                ));
            }
            return Ok(outcome(
                WatchAction::Confirmed,
                channel_id,
                None,
                observation.peak_height,
            ));
        }
        return Err(WatcherError::UnexpectedSpend);
    }
    if u64::from(observation.peak_height) < decode_height(&args.refund_height)? {
        return Ok(outcome(
            WatchAction::Idle,
            channel_id,
            None,
            observation.peak_height,
        ));
    }
    if observation
        .funding_coin
        .as_ref()
        .is_none_or(|coin| coin.spent)
    {
        return Err(WatcherError::UnexpectedSpend);
    }
    if record.state != ChannelState::Refundable {
        store.mark_refundable(channel_id)?;
    }
    let key = refund_key(channel_id);
    let job = if let Some(job) = store.load_broadcast(&key)? {
        job
    } else {
        let bundle = build_refund_bundle(
            observation
                .funding_coin
                .as_ref()
                .ok_or(WatcherError::FundingCoinUnavailable)?
                .coin,
            args,
            user_secret_key,
            agg_sig_me_additional_data,
        )?;
        store.prepare_broadcast(&BroadcastRequest {
            idempotency_key: &key,
            channel_id,
            kind: BroadcastKind::Refund,
            bundle: &bundle,
            funding_coin_id,
            fee: None,
            fee_coin_id: None,
        })?
    };
    let tracked_observation = node.observe(job.spend_bundle_id, funding_coin_id).await?;
    store.record_chain_observation_with_reorg(channel_id, &tracked_observation, job.fee)?;
    let bundle = SpendBundle::from_bytes(&job.spend_bundle)
        .map_err(|error| WatcherError::BundleDecode(error.to_string()))?;
    if let BroadcastState::Submitted | BroadcastState::Pending = job.state
        && matches!(
            node.mempool_status(job.spend_bundle_id).await?,
            crate::MempoolStatus::Pending { .. }
        )
    {
        return Ok(outcome(
            WatchAction::Idle,
            channel_id,
            Some(job.spend_bundle_id),
            observation.peak_height,
        ));
    }
    store.record_broadcast_attempt(&key, BroadcastState::Pending, None)?;
    match node.broadcast(bundle).await {
        Ok(tx_id) => {
            store.update_broadcast_state(&key, BroadcastState::Submitted, None)?;
            if store.load_channel(channel_id)?.state == ChannelState::Refundable {
                store.mark_refund_submitted(channel_id)?;
            }
            Ok(outcome(
                WatchAction::BroadcastSubmitted,
                channel_id,
                Some(tx_id),
                observation.peak_height,
            ))
        }
        Err(error) => {
            store.update_broadcast_state(
                &key,
                BroadcastState::Rejected,
                Some(&error.to_string()),
            )?;
            Err(error.into())
        }
    }
}

fn claim_key(channel_id: Bytes32) -> String {
    format!("claim:{}", hex::encode(channel_id))
}

fn refund_key(channel_id: Bytes32) -> String {
    format!("refund:{}", hex::encode(channel_id))
}

pub(crate) fn decode_height(bytes: &chia_protocol::Bytes) -> Result<u64, WatcherError> {
    let bytes = bytes.as_ref();
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| WatcherError::InvalidHeight)?,
    ))
}

fn has_confirmation_depth(peak: u32, confirmed: u32, depth: u32) -> bool {
    peak.saturating_sub(confirmed).saturating_add(1) >= depth.max(1)
}

fn outcome(
    action: WatchAction,
    channel_id: Bytes32,
    spend_bundle_id: Option<Bytes32>,
    peak_height: u32,
) -> WatchOutcome {
    WatchOutcome {
        action,
        channel_id: format!("0x{}", hex::encode(channel_id)),
        spend_bundle_id: spend_bundle_id.map(|id| format!("0x{}", hex::encode(id))),
        peak_height,
    }
}

pub fn run_role_cli(role: &str) -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "help" {
        println!(
            "wall-hub-{role}\n\n  artifact encode <Invoice|Intent|Voucher> <payload_hex> <channel_id> <idempotency_key>\n  artifact decode <json>\n  watch <config.json> [--once]\n  metrics <db_path>"
        );
        return Ok(());
    }
    if args[0] == "watch" && (args.len() == 2 || args.len() == 3) {
        let once = args.get(2).is_some_and(|value| value == "--once");
        if args.get(2).is_some() && !once {
            return Err("watch accepts only the optional --once flag".to_string());
        }
        return run_watch_cli(role, Path::new(&args[1]), once);
    }
    if args[0] == "metrics" && args.len() == 2 {
        let store = ChannelStore::open(&args[1]).map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&store.metrics().map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if args[0] != "artifact" || args.len() < 3 {
        return Err("use `artifact encode` or `artifact decode`; see --help".to_string());
    }
    match args[1].as_str() {
        "encode" if args.len() == 6 => {
            let payload = hex::decode(&args[3]).map_err(|error| error.to_string())?;
            let channel_id = parse_bytes32(&args[4]).map_err(|error| error.to_string())?;
            println!(
                "{}",
                encode_artifact(
                    &args[2],
                    &payload,
                    format!("{role}-cli"),
                    &args[5],
                    channel_id
                )
                .map_err(|error| error.to_string())?
            );
            Ok(())
        }
        "decode" if args.len() == 3 => {
            let (envelope, _) = decode_artifact(&args[2]).map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&envelope).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        _ => Err("invalid artifact arguments; see --help".to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct WatchConfig {
    pub db_path: String,
    pub network: Option<String>,
    pub channel_id: String,
    pub genesis_challenge: String,
    pub user_public_key: String,
    pub hub_public_key: String,
    pub user_puzzle_hash: String,
    pub claim_before_height: u64,
    pub refund_height: u64,
    pub rpc_url: Option<String>,
    pub safety_margin: Option<u32>,
    pub confirmation_depth: Option<u32>,
    pub poll_interval_secs: Option<u64>,
    pub max_iterations: Option<u32>,
    pub user_secret_key: Option<String>,
    pub agg_sig_me_additional_data: Option<String>,
}

pub fn run_watch_cli(role: &str, config_path: &Path, once: bool) -> Result<(), String> {
    let config: WatchConfig =
        serde_json::from_str(&fs::read_to_string(config_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let channel_id = parse_config_bytes32(&config.channel_id)?;
    let genesis_challenge = parse_config_bytes32(&config.genesis_challenge)?;
    let user_puzzle_hash = parse_config_bytes32(&config.user_puzzle_hash)?;
    let user_public_key = parse_public_key(&config.user_public_key)?;
    let hub_public_key = parse_public_key(&config.hub_public_key)?;
    let args = ChannelArgs::new(
        user_public_key,
        hub_public_key,
        user_puzzle_hash,
        genesis_challenge,
        config.claim_before_height,
        config.refund_height,
    )
    .map_err(|error| error.to_string())?;
    let network = config.network.as_deref().unwrap_or("mainnet");
    let (rpc_config, default_additional_data, expected_genesis_challenge) = match network {
        "mainnet" => (
            crate::ChiaRpcConfig::PublicMainnet {
                base_url: config
                    .rpc_url
                    .unwrap_or_else(|| "https://api.coinset.org".to_string()),
            },
            MAINNET_CONSTANTS.agg_sig_me_additional_data,
            MAINNET_CONSTANTS.genesis_challenge,
        ),
        "testnet11" => (
            crate::ChiaRpcConfig::PublicTestnet11 {
                base_url: config
                    .rpc_url
                    .unwrap_or_else(|| "https://testnet11.api.coinset.org".to_string()),
            },
            TESTNET11_CONSTANTS.agg_sig_me_additional_data,
            TESTNET11_CONSTANTS.genesis_challenge,
        ),
        other => return Err(format!("unsupported network: {other}")),
    };
    if genesis_challenge != expected_genesis_challenge {
        return Err(format!(
            "genesis_challenge does not match {network} network"
        ));
    }
    let node =
        ChiaNode::connect(rpc_config, genesis_challenge).map_err(|error| error.to_string())?;
    let mut store = ChannelStore::open(&config.db_path).map_err(|error| error.to_string())?;
    let confirmation_depth = config.confirmation_depth.unwrap_or(3);
    let safety_margin = config.safety_margin.unwrap_or(5);
    let interval = Duration::from_secs(config.poll_interval_secs.unwrap_or(10));
    let max_iterations = if once { Some(1) } else { config.max_iterations };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let mut iteration = 0_u32;
    loop {
        let result = if role == "merchant" {
            runtime.block_on(merchant_watch_once(
                &mut store,
                &node,
                channel_id,
                &args,
                safety_margin,
                confirmation_depth,
            ))
        } else if role == "user" {
            let secret = config
                .user_secret_key
                .as_deref()
                .ok_or_else(|| "user_secret_key is required for the user watcher".to_string())
                .and_then(parse_secret_key)?;
            let additional_data = config
                .agg_sig_me_additional_data
                .as_deref()
                .map(parse_config_bytes32)
                .transpose()?
                .unwrap_or(default_additional_data);
            runtime.block_on(user_refund_watch_once(
                &mut store,
                &node,
                channel_id,
                &args,
                &secret,
                additional_data,
                confirmation_depth,
            ))
        } else {
            return Err("watch is supported only by the user and merchant roles".to_string());
        };
        let output = result.map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string(&output).map_err(|error| error.to_string())?
        );
        iteration = iteration.saturating_add(1);
        if max_iterations.is_some_and(|limit| iteration >= limit) {
            break;
        }
        std::thread::sleep(interval);
    }
    Ok(())
}

fn parse_config_bytes32(value: &str) -> Result<Bytes32, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "expected 32-byte hex".to_string())?;
    Ok(Bytes32::from(bytes))
}

fn parse_public_key(value: &str) -> Result<PublicKey, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    let bytes: [u8; 48] = bytes
        .try_into()
        .map_err(|_| "expected 48-byte hex".to_string())?;
    PublicKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

fn parse_secret_key(value: &str) -> Result<SecretKey, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "expected 32-byte hex".to_string())?;
    SecretKey::from_bytes(&bytes).map_err(|error| error.to_string())
}

impl fmt::Display for WatchAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self)
    }
}

#[allow(dead_code)]
fn _watcher_signature_markers() -> Duration {
    Duration::from_secs(1)
}
