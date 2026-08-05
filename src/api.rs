use std::{collections::HashMap, net::SocketAddr, path::Path, sync::Arc, time::{SystemTime, UNIX_EPOCH}};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chia_bls::{PublicKey, SecretKey};
use chia_protocol::Bytes32;
use chia_sdk_types::MAINNET_CONSTANTS;
use chia_sdk_utils::Address;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    API_PROTOCOL_VERSION, API_SCHEMA_VERSION, ChannelArgs, ChannelState, ChannelStore, ChiaNode,
    ChiaNodeError, ChiaRpcConfig, FUNDING_AMOUNT, InvoiceFields, MAX_PROTOCOL_U64, MERCHANT_AMOUNT,
    ChannelTermsV2, MerchantInvoice, NoiseHubSessions, PaymentIntent, PaymentVoucher,
    StateStoreError, WalletConnectState, WalletConnectStore, puzzle_reveal, puzzle_reveal_v2,
    sign_connect_uri,
};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct HubApiConfig {
    pub listen_addr: String,
    pub db_path: String,
    pub rpc_url: String,
    pub hub_secret_key: String,
    pub api_key: String,
    pub agg_sig_me_additional_data: Option<String>,
    pub connect_public_base_url: Option<String>,
    pub connect_hub_key_id: Option<String>,
    pub connect_request_secret_key: Option<String>,
    pub connect_noise_private_key: Option<String>,
}

#[derive(Clone)]
struct ApiState {
    runtime: Arc<Mutex<HubRuntime>>,
    node: Arc<ChiaNode>,
    api_key: Arc<String>,
    hub_public_key: String,
    noise_sessions: Arc<Mutex<NoiseHubSessions>>,
    noise_bindings: Arc<Mutex<HashMap<String, String>>>,
    noise_configured: bool,
}

struct HubRuntime {
    hub_secret_key: SecretKey,
    store: ChannelStore,
    agg_sig_me_additional_data: Bytes32,
    wallet_connect_store: WalletConnectStore,
    connect_request_secret_key: Option<SecretKey>,
    connect_hub_key_id: Option<String>,
    connect_public_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetails,
}

#[derive(Debug, Serialize)]
struct ErrorDetails {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct HttpError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl HttpError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "UNAUTHORIZED",
            message: "missing or invalid X-API-Key".to_string(),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorDetails {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct ChannelRequest {
    pub user_public_key: String,
    pub hub_public_key: String,
    pub user_puzzle_hash: String,
    pub genesis_challenge: String,
    pub funding_amount: Option<u64>,
    pub claim_before_height: u64,
    pub refund_height: u64,
}

impl ChannelRequest {
    fn channel_args(&self) -> Result<ChannelArgs, HttpError> {
        ChannelArgs::new(
            parse_public_key(&self.user_public_key, "user_public_key")?,
            parse_public_key(&self.hub_public_key, "hub_public_key")?,
            parse_bytes32(&self.user_puzzle_hash, "user_puzzle_hash")?,
            parse_bytes32(&self.genesis_challenge, "genesis_challenge")?,
            self.claim_before_height,
            self.refund_height,
        )
        .map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))
    }

    fn funding_amount(&self) -> Result<u64, HttpError> {
        let funding_amount = self.funding_amount.unwrap_or(FUNDING_AMOUNT);
        if funding_amount > MAX_PROTOCOL_U64 || funding_amount <= MERCHANT_AMOUNT {
            return Err(HttpError::bad_request(
                "INVALID_FUNDING_AMOUNT",
                "funding_amount must be greater than merchant amount and fit the protocol range",
            ));
        }
        Ok(funding_amount)
    }
}

#[derive(Debug, Deserialize)]
pub struct InvoiceRequest {
    pub request_id: String,
    pub idempotency_key: String,
    pub channel: ChannelRequest,
    pub funding_coin_id: Option<String>,
    pub order_id: String,
    pub merchant_puzzle_hash: String,
    pub payment_expiry_height: u64,
    pub invoice_nonce: String,
}

#[derive(Debug, Serialize)]
pub struct InvoiceResponse {
    pub request_id: String,
    pub idempotency_key: String,
    pub channel_id: String,
    pub invoice_hash: String,
    pub hub_public_key: String,
    pub invoice_hex: String,
}

#[derive(Debug, Deserialize)]
pub struct VoucherRequest {
    pub request_id: String,
    pub idempotency_key: String,
    pub channel: ChannelRequest,
    pub invoice_hex: String,
    pub intent_hex: String,
}

#[derive(Debug, Serialize)]
pub struct VoucherResponse {
    pub request_id: String,
    pub idempotency_key: String,
    pub channel_id: String,
    pub hub_public_key: String,
    pub voucher_hex: String,
}

#[derive(Debug, Serialize)]
pub struct ChannelAddressResponse {
    pub channel_address: String,
    pub channel_puzzle_hash: String,
    pub funding_amount: u64,
    pub user_remaining_amount: u64,
    pub claim_before_height: u64,
    pub refund_height: u64,
}

#[derive(Debug, Deserialize)]
pub struct ChannelFundingRequest {
    pub channel: ChannelRequest,
}

#[derive(Debug, Serialize)]
pub struct FundingCoinCandidate {
    pub funding_coin_id: String,
    pub amount: u64,
    pub confirmed_height: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ChannelFundingResponse {
    pub status: String,
    pub channel_address: String,
    pub channel_puzzle_hash: String,
    pub funding_amount: u64,
    pub funding_coin_id: Option<String>,
    pub confirmed_height: Option<u32>,
    pub peak_height: u32,
    pub candidates: Vec<FundingCoinCandidate>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    role: &'static str,
    network: &'static str,
    schema_version: u16,
    protocol_version: u16,
    genesis_challenge: String,
    hub_public_key: String,
}

#[derive(Debug, Deserialize)]
struct CreateWalletConnectRequest {
    asset_id: String,
    amount_mojos: String,
    refund_delay_blocks: u64,
    network: String,
}

#[derive(Debug, Serialize)]
struct WalletConnectDisplay {
    amount_xch: String,
    refund_delay_blocks: u64,
    estimated_refund_time: String,
}

#[derive(Debug, Serialize)]
struct CreateWalletConnectResponse {
    request_id: String,
    connect_uri: String,
    expires_at: u64,
    display: WalletConnectDisplay,
}

#[derive(Debug, Serialize)]
struct WalletConnectStatusResponse {
    request_id: String,
    state: &'static str,
    expires_at: u64,
    transaction_id: Option<String>,
    confirmations: u32,
}

#[derive(Debug, Deserialize)]
struct RelayFrameRequest { session_id: String, message: String }
#[derive(Debug, Serialize)]
struct RelayFrameResponse { message: String }

#[derive(Debug, Deserialize)]
struct WalletHello {
    protocol: String,
    version: u16,
    #[serde(rename = "type")]
    message_type: String,
    request_id: String,
    session_id: String,
    wallet_public_key: String,
    channel: WalletHelloChannel,
    nonce: String,
}
#[derive(Debug, Deserialize)]
struct WalletHelloChannel { user_public_key: String, user_puzzle_hash: String }
#[derive(Debug, Deserialize)]
struct FundingStatus { #[serde(rename = "type")] message_type: String, request_id: String, status: String, transaction_id: Option<String> }

#[derive(Debug, Serialize)]
struct FundingRequestFinal {
    protocol: &'static str, version: u16, #[serde(rename = "type")] message_type: &'static str,
    request_id: String, session_id: String, created_at: u64, expires_at: u64, network: &'static str,
    hub_key_id: String, wallet_public_key: String, wallet_nonce: String,
    funding: FundingRequestFunding, channel: FundingRequestChannel, hub_signature: String,
}
#[derive(Debug, Serialize)]
struct FundingRequestFunding { asset_id: &'static str, amount_mojos: String, required_confirmations: u32 }
#[derive(Debug, Serialize)]
struct FundingRequestChannel { protocol_version: u16, hub_public_key: String, user_public_key: String, user_puzzle_hash: String, claim_before_height: u64, refund_height: u64, funding_puzzle_hash: String, channel_terms_hash: String }

#[derive(Debug, Serialize)]
struct HubKeyRegistryResponse {
    protocol: &'static str,
    keys: Vec<HubKeyRegistryEntry>,
}

#[derive(Debug, Serialize)]
struct HubKeyRegistryEntry {
    key_id: String,
    public_key: String,
    allowed_origin: Option<String>,
}

pub async fn run_hub_api(config_path: impl AsRef<Path>) -> Result<(), String> {
    let config: HubApiConfig = serde_json::from_str(
        &std::fs::read_to_string(config_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if config.api_key.trim().is_empty() {
        return Err("api_key must not be empty".to_string());
    }

    let listen_addr: SocketAddr = config
        .listen_addr
        .parse()
        .map_err(|error| format!("invalid listen_addr: {error}"))?;
    let node = Arc::new(
        ChiaNode::connect(
            ChiaRpcConfig::PublicMainnet {
                base_url: config.rpc_url.clone(),
            },
            MAINNET_CONSTANTS.genesis_challenge,
        )
        .map_err(|error| error.to_string())?,
    );
    let node_status = node.status().await.map_err(|error| error.to_string())?;
    println!(
        "mainnet RPC connected: peak_height={}, synced={}",
        node_status.peak_height, node_status.synced
    );
    let hub_secret_key = parse_secret_key(&config.hub_secret_key)?;
    let hub_public_key = format!("0x{}", hex::encode(hub_secret_key.public_key().to_bytes()));
    let additional_data = config
        .agg_sig_me_additional_data
        .as_deref()
        .map(|value| parse_bytes32(value, "agg_sig_me_additional_data"))
        .transpose()
        .map_err(|error| error.message)?
        .unwrap_or(MAINNET_CONSTANTS.agg_sig_me_additional_data);
    let store = ChannelStore::open(&config.db_path).map_err(|error| error.to_string())?;
    let wallet_connect_store = WalletConnectStore::open(&config.db_path).map_err(|error| error.to_string())?;
    let connect_request_secret_key = config
        .connect_request_secret_key
        .as_deref()
        .map(parse_secret_key)
        .transpose()?;
    let connect_noise_private_key = config.connect_noise_private_key.as_deref().map(parse_raw_32).transpose()?;
    let noise_configured = connect_noise_private_key.is_some();

    let state = ApiState {
        runtime: Arc::new(Mutex::new(HubRuntime {
            hub_secret_key,
            store,
            agg_sig_me_additional_data: additional_data,
            wallet_connect_store,
            connect_request_secret_key,
            connect_hub_key_id: config.connect_hub_key_id,
            connect_public_base_url: config.connect_public_base_url,
        })),
        node,
        api_key: Arc::new(config.api_key),
        hub_public_key,
        noise_sessions: Arc::new(Mutex::new(NoiseHubSessions::new(connect_noise_private_key.unwrap_or([0; 32])))),
        noise_bindings: Arc::new(Mutex::new(HashMap::new())),
        noise_configured,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/merchant", get(merchant_page))
        .route("/v1/channel-address", post(derive_channel_address))
        .route("/v1/channel-funding", post(channel_funding_status))
        .route("/v1/invoices", post(issue_invoice))
        .route("/v1/vouchers", post(issue_voucher))
        .route("/v1/wallet-connect/requests", post(create_wallet_connect_request))
        .route("/v1/wallet-connect/requests/{request_id}", get(wallet_connect_status))
        .route("/.well-known/xhub-connect-keys.json", get(wallet_connect_keys))
        .route("/v1/wallet-connect/relay/{request_id}/handshake", post(wallet_connect_handshake))
        .route("/v1/wallet-connect/relay/{request_id}/messages", post(wallet_connect_message))
        .route("/v1/wallet-connect/requests/{request_id}/verify", post(verify_wallet_connect_funding))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .map_err(|error| error.to_string())?;
    println!("hub-api listening on http://{listen_addr}");
    axum::serve(listener, app)
        .await
        .map_err(|error| error.to_string())
}

async fn wallet_connect_keys(
    State(state): State<ApiState>,
) -> Result<Json<HubKeyRegistryResponse>, HttpError> {
    let runtime = state.runtime.lock().await;
    let key_id = runtime.connect_hub_key_id.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect Hub key id is not configured"))?;
    let request_key = runtime.connect_request_secret_key.as_ref().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect request signing key is not configured"))?;
    Ok(Json(HubKeyRegistryResponse {
        protocol: "hubwallet-connect",
        keys: vec![HubKeyRegistryEntry {
            key_id,
            public_key: hex::encode(request_key.public_key().to_bytes()),
            allowed_origin: runtime.connect_public_base_url.clone(),
        }],
    }))
}

async fn wallet_connect_handshake(
    State(state): State<ApiState>, AxumPath(request_id): AxumPath<String>, Json(frame): Json<RelayFrameRequest>,
) -> Result<Json<RelayFrameResponse>, HttpError> {
    if !state.noise_configured { return Err(HttpError::bad_request("CONNECT_NOT_CONFIGURED", "Noise private key is not configured")); }
    if frame.session_id.is_empty() || frame.session_id.len() > 128 { return Err(HttpError::bad_request("INVALID_SESSION", "session_id is invalid")); }
    let runtime = state.runtime.lock().await;
    let record = runtime.wallet_connect_store.load(&request_id).map_err(wallet_connect_error)?.ok_or_else(|| HttpError::bad_request("REQUEST_NOT_FOUND", "wallet-connect request was not found"))?;
    drop(runtime);
    if record.expires_at <= unix_now() || !matches!(record.state, WalletConnectState::Created | WalletConnectState::Paired) { return Err(HttpError::bad_request("REQUEST_UNAVAILABLE", "wallet-connect request is unavailable")); }
    let key = format!("{request_id}:{}", frame.session_id);
    state.noise_bindings.lock().await.insert(key.clone(), request_id);
    let payload = decode_frame(&frame.message)?;
    let response = state.noise_sessions.lock().await.handshake_frame(&key, &payload).map_err(|error| HttpError::bad_request("NOISE_HANDSHAKE", error.to_string()))?.ok_or_else(|| HttpError::bad_request("NOISE_HANDSHAKE", "handshake produced no response"))?;
    Ok(Json(RelayFrameResponse { message: URL_SAFE_NO_PAD.encode(response) }))
}

async fn wallet_connect_message(
    State(state): State<ApiState>, AxumPath(request_id): AxumPath<String>, Json(frame): Json<RelayFrameRequest>,
) -> Result<Json<RelayFrameResponse>, HttpError> {
    let key = format!("{request_id}:{}", frame.session_id);
    if state.noise_bindings.lock().await.get(&key).map(String::as_str) != Some(request_id.as_str()) { return Err(HttpError::bad_request("UNKNOWN_SESSION", "session is not bound to this request")); }
    let plaintext = state.noise_sessions.lock().await.receive(&key, &decode_frame(&frame.message)?).map_err(|error| HttpError::bad_request("NOISE_MESSAGE", error.to_string()))?;
    let message_type = serde_json::from_slice::<serde_json::Value>(&plaintext).ok().and_then(|value| value.get("type").and_then(serde_json::Value::as_str).map(str::to_owned));
    if message_type.as_deref() == Some("funding_status") {
        let status: FundingStatus = serde_json::from_slice(&plaintext).map_err(|_| HttpError::bad_request("INVALID_FUNDING_STATUS", "invalid funding status"))?;
        if status.message_type != "funding_status" || status.request_id != request_id || status.status != "broadcast" { return Err(HttpError::bad_request("INVALID_FUNDING_STATUS", "only a bound broadcast status is accepted")); }
        let tx_id = status.transaction_id.as_deref().ok_or_else(|| HttpError::bad_request("INVALID_FUNDING_STATUS", "broadcast requires transaction_id")).and_then(|value| parse_bytes32(value, "transaction_id"))?;
        let mut runtime = state.runtime.lock().await;
        runtime.wallet_connect_store.record_broadcast(&request_id, tx_id).map_err(wallet_connect_error)?;
        drop(runtime);
        let encrypted = state.noise_sessions.lock().await.send(&key, br#"{"type":"funding_status_ack","status":"recorded"}"#).map_err(|error| HttpError::bad_request("NOISE_MESSAGE", error.to_string()))?;
        return Ok(Json(RelayFrameResponse { message: URL_SAFE_NO_PAD.encode(encrypted) }));
    }
    let hello: WalletHello = serde_json::from_slice(&plaintext).map_err(|_| HttpError::bad_request("INVALID_WALLET_HELLO", "encrypted message is not a valid wallet_hello"))?;
    validate_wallet_hello(&hello, &request_id, &frame.session_id)?;
    let peak_height = trusted_peak_height(&state).await?;
    let mut runtime = state.runtime.lock().await;
    let request = runtime.wallet_connect_store.pair(&request_id, &hello.session_id, parse_public_key_bytes(&hello.channel.user_public_key)?, parse_bytes32(&hello.channel.user_puzzle_hash, "user_puzzle_hash")?).map_err(wallet_connect_error)?;
    let request_key = runtime.connect_request_secret_key.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect request signing key is not configured"))?;
    let hub_key_id = runtime.connect_hub_key_id.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect Hub key id is not configured"))?;
    let refund_height = peak_height.checked_add(request.refund_delay_blocks).ok_or_else(|| HttpError::bad_request("INVALID_HEIGHT", "refund height overflow"))?;
    let claim_before_height = refund_height.checked_sub(1).ok_or_else(|| HttpError::bad_request("INVALID_HEIGHT", "refund height is too low"))?;
    let user_key = parse_public_key(&hello.channel.user_public_key, "user_public_key")?;
    let user_puzzle_hash = parse_bytes32(&hello.channel.user_puzzle_hash, "user_puzzle_hash")?;
    let terms = ChannelTermsV2::new(user_key, runtime.hub_secret_key.public_key(), user_puzzle_hash, MAINNET_CONSTANTS.genesis_challenge, runtime.agg_sig_me_additional_data, request.amount_mojos, claim_before_height, refund_height).map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))?;
    let (funding_puzzle_hash, _) = puzzle_reveal_v2(&terms).map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))?;
    runtime.wallet_connect_store.authorize_funding(&request_id, funding_puzzle_hash).map_err(wallet_connect_error)?;
    let mut final_request = FundingRequestFinal {
        protocol: "hubwallet-connect", version: 1, message_type: "funding_request_final", request_id: request_id.clone(), session_id: frame.session_id.clone(), created_at: unix_now(), expires_at: request.expires_at, network: "mainnet", hub_key_id, wallet_public_key: hello.wallet_public_key.clone(), wallet_nonce: hello.nonce.clone(),
        funding: FundingRequestFunding { asset_id: "xch", amount_mojos: request.amount_mojos.to_string(), required_confirmations: 1 },
        channel: FundingRequestChannel { protocol_version: 2, hub_public_key: hex::encode(runtime.hub_secret_key.public_key().to_bytes()), user_public_key: hello.channel.user_public_key.clone(), user_puzzle_hash: hello.channel.user_puzzle_hash.clone(), claim_before_height, refund_height, funding_puzzle_hash: hex::encode(funding_puzzle_hash), channel_terms_hash: hex::encode(terms.channel_terms_hash()) }, hub_signature: String::new(),
    };
    final_request.hub_signature = URL_SAFE_NO_PAD.encode(chia_bls::sign(&request_key, funding_request_signature_hash(&final_request).as_ref()).to_bytes());
    let encoded = serde_json::to_vec(&final_request).map_err(|_| HttpError::bad_request("SERIALIZATION_ERROR", "failed to serialize final request"))?;
    drop(runtime);
    let encrypted = state.noise_sessions.lock().await.send(&key, &encoded).map_err(|error| HttpError::bad_request("NOISE_MESSAGE", error.to_string()))?;
    Ok(Json(RelayFrameResponse { message: URL_SAFE_NO_PAD.encode(encrypted) }))
}

async fn verify_wallet_connect_funding(
    State(state): State<ApiState>, AxumPath(request_id): AxumPath<String>,
) -> Result<Json<WalletConnectStatusResponse>, HttpError> {
    let record = { state.runtime.lock().await.wallet_connect_store.load(&request_id).map_err(wallet_connect_error)?.ok_or_else(|| HttpError::bad_request("REQUEST_NOT_FOUND", "wallet-connect request was not found"))? };
    let funding_puzzle_hash = record.funding_puzzle_hash.ok_or_else(|| HttpError::bad_request("FUNDING_NOT_AUTHORIZED", "funding parameters have not been authorized"))?;
    if !matches!(record.state, WalletConnectState::Broadcast | WalletConnectState::PendingConfirmation | WalletConnectState::Active | WalletConnectState::Reorged) { return Err(HttpError::bad_request("FUNDING_NOT_BROADCAST", "wallet has not reported broadcast")); }
    let node_status = state.node.status().await.map_err(node_error)?;
    let candidates = state.node.get_unspent_coins(funding_puzzle_hash, record.amount_mojos).await.map_err(node_error)?.into_iter().filter(|coin| coin.coin.amount == record.amount_mojos).collect::<Vec<_>>();
    let prior_coin = if candidates.is_empty() {
        match record.funding_coin_id {
            Some(coin_id) => state.node.get_coin(coin_id).await.map_err(node_error)?,
            None => None,
        }
    } else { None };
    let mut runtime = state.runtime.lock().await;
    let next = match candidates.as_slice() {
        [coin] if coin.confirmed_block_index > 0 => {
            let confirmations = node_status.peak_height.saturating_sub(coin.confirmed_block_index).saturating_add(1);
            runtime.wallet_connect_store.observe_funding(&request_id, coin.coin.coin_id(), confirmations >= 1, false).map_err(wallet_connect_error)?
        }
        [coin] => runtime.wallet_connect_store.observe_funding(&request_id, coin.coin.coin_id(), false, false).map_err(wallet_connect_error)?,
        [] if record.state == WalletConnectState::Active && prior_coin.is_none() && record.funding_coin_id.is_some() => runtime.wallet_connect_store.observe_funding(&request_id, record.funding_coin_id.expect("checked"), false, true).map_err(wallet_connect_error)?,
        [] if prior_coin.as_ref().is_some_and(|coin| coin.spent) => return Err(HttpError::bad_request("FUNDING_SPENT", "funding coin is spent and cannot be reactivated")),
        [] => record,
        _ => return Err(HttpError::bad_request("AMBIGUOUS_FUNDING", "multiple exact funding coins match this request")),
    };
    Ok(Json(WalletConnectStatusResponse { request_id: next.request_id, state: next.state.as_str(), expires_at: next.expires_at, transaction_id: next.transaction_id.map(hex::encode), confirmations: 0 }))
}

async fn create_wallet_connect_request(
    State(state): State<ApiState>,
    Json(request): Json<CreateWalletConnectRequest>,
) -> Result<Json<CreateWalletConnectResponse>, HttpError> {
    if request.asset_id != "xch" || request.network != "mainnet" {
        return Err(HttpError::bad_request("UNSUPPORTED_ASSET_OR_NETWORK", "only mainnet XCH is supported"));
    }
    let amount_mojos = request.amount_mojos.parse::<u64>().map_err(|_| HttpError::bad_request("INVALID_AMOUNT", "amount_mojos must be an unsigned decimal integer"))?;
    if amount_mojos == 0 || amount_mojos > MAX_PROTOCOL_U64 {
        return Err(HttpError::bad_request("INVALID_AMOUNT", "amount_mojos is out of range"));
    }
    if request.refund_delay_blocks < 20 {
        return Err(HttpError::bad_request("INVALID_REFUND_DELAY", "refund_delay_blocks must be at least 20"));
    }
    let expires_at = unix_now().checked_add(300).ok_or_else(|| HttpError::bad_request("CLOCK_ERROR", "server clock overflow"))?;
    let mut runtime = state.runtime.lock().await;
    let public_base = runtime.connect_public_base_url.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect public base URL is not configured"))?;
    let key_id = runtime.connect_hub_key_id.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect Hub key id is not configured"))?;
    let request_key = runtime.connect_request_secret_key.clone().ok_or_else(|| HttpError::bad_request("CONNECT_NOT_CONFIGURED", "wallet-connect request signing key is not configured"))?;
    let stored = runtime.wallet_connect_store.create(amount_mojos, request.refund_delay_blocks, "mainnet", expires_at).map_err(wallet_connect_error)?;
    let request_uri = format!("{}/v1/wallet-connect/requests/{}", public_base.trim_end_matches('/'), stored.request_id);
    let signature = sign_connect_uri(&request_key, &request_uri, &stored.request_id, expires_at, &key_id).map_err(wallet_connect_error)?;
    let connect_uri = format!("hubwallet://connect?v=1&request_uri={}&request_id={}&expires_at={}&hub_key_id={}&sig={}", percent_encode(&request_uri), stored.request_id, expires_at, percent_encode(&key_id), signature);
    Ok(Json(CreateWalletConnectResponse {
        request_id: stored.request_id,
        connect_uri,
        expires_at,
        display: WalletConnectDisplay { amount_xch: format_xch(amount_mojos), refund_delay_blocks: request.refund_delay_blocks, estimated_refund_time: format!("about {} blocks", request.refund_delay_blocks) },
    }))
}

async fn wallet_connect_status(
    State(state): State<ApiState>,
    AxumPath(request_id): AxumPath<String>,
) -> Result<Json<WalletConnectStatusResponse>, HttpError> {
    let mut runtime = state.runtime.lock().await;
    let record = runtime.wallet_connect_store.load(&request_id).map_err(wallet_connect_error)?.ok_or_else(|| HttpError::bad_request("REQUEST_NOT_FOUND", "wallet-connect request was not found"))?;
    let record = if record.expires_at <= unix_now() && !matches!(record.state, WalletConnectState::Active | WalletConnectState::Cancelled | WalletConnectState::Expired | WalletConnectState::Rejected | WalletConnectState::Failed) {
        runtime.wallet_connect_store.transition(&request_id, WalletConnectState::Expired).unwrap_or(record)
    } else { record };
    Ok(Json(WalletConnectStatusResponse { request_id: record.request_id, state: record.state.as_str(), expires_at: record.expires_at, transaction_id: None, confirmations: 0 }))
}

async fn health(State(state): State<ApiState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        role: "hub",
        network: "mainnet",
        schema_version: API_SCHEMA_VERSION,
        protocol_version: API_PROTOCOL_VERSION,
        genesis_challenge: format!("0x{}", hex::encode(MAINNET_CONSTANTS.genesis_challenge)),
        hub_public_key: state.hub_public_key,
    })
}

async fn derive_channel_address(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ChannelRequest>,
) -> Result<Json<ChannelAddressResponse>, HttpError> {
    authorize(&headers, &state)?;
    let args = request.channel_args()?;
    let funding_amount = request.funding_amount()?;
    validate_mainnet(&args)?;
    let (puzzle_hash, _) = puzzle_reveal(&args)
        .map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))?;
    let channel_address = Address::new(puzzle_hash, "xch".to_string())
        .encode()
        .map_err(|error| HttpError::bad_request("ADDRESS_ENCODING", error.to_string()))?;
    Ok(Json(ChannelAddressResponse {
        channel_address,
        channel_puzzle_hash: format!("0x{}", hex::encode(puzzle_hash)),
        funding_amount,
        user_remaining_amount: funding_amount - MERCHANT_AMOUNT,
        claim_before_height: decode_fixed_height(&args.claim_before_height),
        refund_height: decode_fixed_height(&args.refund_height),
    }))
}

async fn channel_funding_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ChannelFundingRequest>,
) -> Result<Json<ChannelFundingResponse>, HttpError> {
    authorize(&headers, &state)?;
    let args = request.channel.channel_args()?;
    let funding_amount = request.channel.funding_amount()?;
    validate_mainnet(&args)?;
    let (puzzle_hash, _) = puzzle_reveal(&args)
        .map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))?;
    let channel_address = Address::new(puzzle_hash, "xch".to_string())
        .encode()
        .map_err(|error| HttpError::bad_request("ADDRESS_ENCODING", error.to_string()))?;
    let peak_height = trusted_peak_height(&state).await? as u32;
    let records = state
        .node
        .get_unspent_coins(puzzle_hash, funding_amount)
        .await
        .map_err(node_error)?;
    let candidates = records
        .into_iter()
        .filter(|record| record.coin.amount == funding_amount)
        .map(|record| FundingCoinCandidate {
            funding_coin_id: format!("0x{}", hex::encode(record.coin.coin_id())),
            amount: record.coin.amount,
            confirmed_height: (record.confirmed_block_index > 0)
                .then_some(record.confirmed_block_index),
        })
        .collect::<Vec<_>>();
    let (status, funding_coin_id, confirmed_height) = funding_status(&candidates);
    Ok(Json(ChannelFundingResponse {
        status,
        channel_address,
        channel_puzzle_hash: format!("0x{}", hex::encode(puzzle_hash)),
        funding_amount,
        funding_coin_id,
        confirmed_height,
        peak_height,
        candidates,
    }))
}

fn funding_status(candidates: &[FundingCoinCandidate]) -> (String, Option<String>, Option<u32>) {
    let confirmed = candidates
        .iter()
        .filter(|candidate| candidate.confirmed_height.is_some())
        .collect::<Vec<_>>();
    match confirmed.as_slice() {
        [candidate] => (
            "FUNDING_CONFIRMED".to_string(),
            Some(candidate.funding_coin_id.clone()),
            candidate.confirmed_height,
        ),
        [] if candidates.is_empty() => ("WAITING_FOR_FUNDING".to_string(), None, None),
        [] => ("PENDING_CONFIRMATION".to_string(), None, None),
        _ => ("AMBIGUOUS_FUNDING".to_string(), None, None),
    }
}

async fn merchant_page() -> Html<&'static str> {
    Html(include_str!("../web/merchant.html"))
}

async fn issue_invoice(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<InvoiceRequest>,
) -> Result<Json<InvoiceResponse>, HttpError> {
    authorize(&headers, &state)?;
    validate_request_meta(&request.request_id, &request.idempotency_key)?;
    let current_height = trusted_peak_height(&state).await?;
    let args = request.channel.channel_args()?;
    let funding_amount = request.channel.funding_amount()?;
    validate_mainnet(&args)?;
    let (channel_puzzle_hash, _) = puzzle_reveal(&args)
        .map_err(|error| HttpError::bad_request("INVALID_CHANNEL", error.to_string()))?;
    let funding_coin_id = resolve_funding_coin(
        &state,
        request.funding_coin_id.as_deref(),
        channel_puzzle_hash,
        funding_amount,
    )
    .await?;
    let fields = InvoiceFields::new(
        args.genesis_challenge,
        funding_coin_id,
        parse_bytes32(&request.order_id, "order_id")?,
        parse_bytes32(&request.merchant_puzzle_hash, "merchant_puzzle_hash")?,
        request.payment_expiry_height,
        parse_bytes32(&request.invoice_nonce, "invoice_nonce")?,
    );
    let runtime = state.runtime.lock().await;
    let invoice = MerchantInvoice::issue_with_signer(fields, &runtime.hub_secret_key);
    invoice
        .verify(&args, funding_coin_id, current_height)
        .map_err(protocol_error)?;
    drop(runtime);

    let mut runtime = state.runtime.lock().await;
    match runtime.store.load_channel(invoice.fields.channel_id) {
        Ok(_) => {}
        Err(StateStoreError::ChannelNotFound) => runtime
            .store
            .create_channel_with_funding_amount(invoice.fields.channel_id, funding_amount)
            .map_err(store_error)?,
        Err(error) => return Err(store_error(error)),
    }
    Ok(Json(InvoiceResponse {
        request_id: request.request_id,
        idempotency_key: request.idempotency_key,
        channel_id: format!("0x{}", hex::encode(invoice.fields.channel_id)),
        invoice_hash: format!("0x{}", hex::encode(invoice.invoice_hash)),
        hub_public_key: state.hub_public_key,
        invoice_hex: hex::encode(invoice.to_bytes()),
    }))
}

async fn resolve_funding_coin(
    state: &ApiState,
    supplied_coin_id: Option<&str>,
    channel_puzzle_hash: Bytes32,
    funding_amount: u64,
) -> Result<Bytes32, HttpError> {
    if let Some(value) = supplied_coin_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let funding_coin_id = parse_bytes32(value, "funding_coin_id")?;
        let record = state
            .node
            .get_coin(funding_coin_id)
            .await
            .map_err(node_error)?
            .ok_or_else(|| {
                HttpError::bad_request("FUNDING_NOT_FOUND", "funding coin was not found")
            })?;
        if record.spent {
            return Err(HttpError::bad_request(
                "FUNDING_SPENT",
                "funding coin is already spent",
            ));
        }
        if record.confirmed_block_index == 0 {
            return Err(HttpError::bad_request(
                "FUNDING_NOT_CONFIRMED",
                "funding coin is not confirmed yet",
            ));
        }
        if record.coin.puzzle_hash != channel_puzzle_hash {
            return Err(HttpError::bad_request(
                "FUNDING_WRONG_PUZZLE",
                "funding coin puzzle hash does not match the channel",
            ));
        }
        if record.coin.amount != funding_amount {
            return Err(HttpError::bad_request(
                "FUNDING_WRONG_AMOUNT",
                "funding coin amount does not match channel funding_amount",
            ));
        }
        return Ok(funding_coin_id);
    }

    let records = state
        .node
        .get_unspent_coins(channel_puzzle_hash, funding_amount)
        .await
        .map_err(node_error)?;
    let confirmed = records
        .into_iter()
        .filter(|record| record.coin.amount == funding_amount && record.confirmed_block_index > 0)
        .collect::<Vec<_>>();
    match confirmed.as_slice() {
        [record] => Ok(record.coin.coin_id()),
        [] => Err(HttpError::bad_request(
            "FUNDING_NOT_CONFIRMED",
            "no unique confirmed funding coin was found for this channel",
        )),
        _ => Err(HttpError::bad_request(
            "AMBIGUOUS_FUNDING",
            "multiple confirmed funding coins match this channel; provide funding_coin_id",
        )),
    }
}

async fn issue_voucher(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<VoucherRequest>,
) -> Result<Json<VoucherResponse>, HttpError> {
    authorize(&headers, &state)?;
    validate_request_meta(&request.request_id, &request.idempotency_key)?;
    let current_height = trusted_peak_height(&state).await?;
    let args = request.channel.channel_args()?;
    let expected_funding_amount = request.channel.funding_amount()?;
    validate_mainnet(&args)?;
    let invoice = MerchantInvoice::from_bytes(
        &hex::decode(&request.invoice_hex)
            .map_err(|error| HttpError::bad_request("INVALID_INVOICE", error.to_string()))?,
    )
    .map_err(protocol_error)?;
    let intent = PaymentIntent::from_bytes(
        &hex::decode(&request.intent_hex)
            .map_err(|error| HttpError::bad_request("INVALID_INTENT", error.to_string()))?,
    )
    .map_err(protocol_error)?;
    if intent
        .commitment
        .merchant_amount
        .checked_add(intent.commitment.user_remaining_amount)
        .is_none_or(|amount| amount != expected_funding_amount)
    {
        return Err(HttpError::bad_request(
            "INVALID_FUNDING_AMOUNT",
            "intent balances do not match channel funding_amount",
        ));
    }
    let channel_id = intent.commitment.channel_id;
    let mut runtime = state.runtime.lock().await;

    if let Ok(record) = runtime.store.load_channel(channel_id)
        && record.state == ChannelState::VoucherIssued
        && let Some(voucher) = record.voucher
    {
        if voucher.intent.to_bytes() == intent.to_bytes() {
            return Ok(Json(voucher_response(
                request.request_id,
                request.idempotency_key,
                voucher,
                &state.hub_public_key,
            )));
        }
        return Err(HttpError::bad_request(
            "IDEMPOTENCY_CONFLICT",
            "channel already has a voucher for a different intent",
        ));
    }

    let hub_secret_key = runtime.hub_secret_key.clone();
    let additional_data = runtime.agg_sig_me_additional_data;
    let voucher = runtime
        .store
        .accept_intent_and_issue_voucher_atomic(
            &intent,
            &invoice,
            &args,
            &hub_secret_key,
            additional_data,
            current_height,
        )
        .map_err(store_error)?;
    Ok(Json(voucher_response(
        request.request_id,
        request.idempotency_key,
        voucher,
        &state.hub_public_key,
    )))
}

fn voucher_response(
    request_id: String,
    idempotency_key: String,
    voucher: PaymentVoucher,
    hub_public_key: &str,
) -> VoucherResponse {
    VoucherResponse {
        request_id,
        idempotency_key,
        channel_id: format!("0x{}", hex::encode(voucher.intent.commitment.channel_id)),
        hub_public_key: hub_public_key.to_string(),
        voucher_hex: hex::encode(voucher.to_bytes()),
    }
}

fn authorize(headers: &HeaderMap, state: &ApiState) -> Result<(), HttpError> {
    let supplied = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if supplied != state.api_key.as_str() {
        return Err(HttpError::unauthorized());
    }
    Ok(())
}

async fn trusted_peak_height(state: &ApiState) -> Result<u64, HttpError> {
    state
        .node
        .status()
        .await
        .map(|status| u64::from(status.peak_height))
        .map_err(node_error)
}

fn validate_mainnet(args: &ChannelArgs) -> Result<(), HttpError> {
    if args.genesis_challenge != MAINNET_CONSTANTS.genesis_challenge {
        return Err(HttpError::bad_request(
            "WRONG_NETWORK",
            "HUB API is configured for Chia mainnet",
        ));
    }
    Ok(())
}

fn validate_request_meta(request_id: &str, idempotency_key: &str) -> Result<(), HttpError> {
    if request_id.trim().is_empty() {
        return Err(HttpError::bad_request(
            "MISSING_REQUEST_ID",
            "request_id must not be empty",
        ));
    }
    if idempotency_key.trim().is_empty() {
        return Err(HttpError::bad_request(
            "MISSING_IDEMPOTENCY_KEY",
            "idempotency_key must not be empty",
        ));
    }
    Ok(())
}

fn decode_fixed_height(value: &[u8]) -> u64 {
    let bytes: [u8; 8] = value
        .try_into()
        .expect("ChannelArgs validates fixed height encoding");
    u64::from_be_bytes(bytes)
}

fn parse_bytes32(value: &str, field: &'static str) -> Result<Bytes32, HttpError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value)
        .map_err(|error| HttpError::bad_request("INVALID_HEX", format!("{field}: {error}")))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        HttpError::bad_request("INVALID_LENGTH", format!("{field} must be 32 bytes"))
    })?;
    Ok(Bytes32::from(bytes))
}

fn parse_public_key(value: &str, field: &'static str) -> Result<PublicKey, HttpError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes = hex::decode(value)
        .map_err(|error| HttpError::bad_request("INVALID_HEX", format!("{field}: {error}")))?;
    let bytes: [u8; 48] = bytes.try_into().map_err(|_| {
        HttpError::bad_request("INVALID_LENGTH", format!("{field} must be 48 bytes"))
    })?;
    PublicKey::from_bytes(&bytes)
        .map_err(|error| HttpError::bad_request("INVALID_PUBLIC_KEY", error.to_string()))
}

fn parse_secret_key(value: &str) -> Result<SecretKey, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    let bytes: [u8; 32] = hex::decode(value)
        .map_err(|error| format!("hub_secret_key is not valid hex: {error}"))?
        .try_into()
        .map_err(|_| "hub_secret_key must be 32 bytes".to_string())?;
    SecretKey::from_bytes(&bytes).map_err(|error| format!("invalid hub_secret_key: {error}"))
}

fn protocol_error(error: crate::ProtocolError) -> HttpError {
    HttpError::bad_request(error.code(), error.to_string())
}

fn store_error(error: StateStoreError) -> HttpError {
    let status = match error {
        StateStoreError::Database(_)
        | StateStoreError::CorruptData(_)
        | StateStoreError::SpendBundleEncoding => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    HttpError {
        status,
        code: error.code(),
        message: error.to_string(),
    }
}

fn node_error(error: ChiaNodeError) -> HttpError {
    HttpError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "RPC_UNAVAILABLE",
        message: error.to_string(),
    }
}

fn wallet_connect_error(error: crate::WalletConnectError) -> HttpError {
    let status = match error {
        crate::WalletConnectError::Database(_) | crate::WalletConnectError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        crate::WalletConnectError::NotFound => StatusCode::NOT_FOUND,
        crate::WalletConnectError::Expired | crate::WalletConnectError::Consumed | crate::WalletConnectError::SessionConflict | crate::WalletConnectError::Transition { .. } | crate::WalletConnectError::Invalid(_) => StatusCode::BAD_REQUEST,
    };
    HttpError { status, code: "WALLET_CONNECT_ERROR", message: error.to_string() }
}

fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

fn percent_encode(value: &str) -> String {
    value.bytes().flat_map(|byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') { vec![byte as char] }
        else { format!("%{byte:02X}").chars().collect() }
    }).collect()
}

fn format_xch(mojos: u64) -> String {
    const MOJOS_PER_XCH: u64 = 1_000_000_000_000;
    let whole = mojos / MOJOS_PER_XCH;
    let fractional = mojos % MOJOS_PER_XCH;
    if fractional == 0 { whole.to_string() } else { format!("{whole}.{fractional:012}").trim_end_matches('0').to_string() }
}

fn decode_frame(value: &str) -> Result<Vec<u8>, HttpError> {
    URL_SAFE_NO_PAD.decode(value).map_err(|_| HttpError::bad_request("INVALID_RELAY_FRAME", "message must be base64url"))
}

fn parse_raw_32(value: &str) -> Result<[u8; 32], String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(value).map_err(|error| format!("invalid 32-byte key: {error}"))?.try_into().map_err(|_| "key must be 32 bytes".to_string())
}

fn parse_public_key_bytes(value: &str) -> Result<[u8; 48], HttpError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    hex::decode(value).map_err(|_| HttpError::bad_request("INVALID_PUBLIC_KEY", "user_public_key must be hex"))?.try_into().map_err(|_| HttpError::bad_request("INVALID_PUBLIC_KEY", "user_public_key must be 48 bytes"))
}

fn validate_wallet_hello(hello: &WalletHello, request_id: &str, session_id: &str) -> Result<(), HttpError> {
    if hello.protocol != "hubwallet-connect" || hello.version != 1 || hello.message_type != "wallet_hello" { return Err(HttpError::bad_request("INVALID_WALLET_HELLO", "unsupported wallet hello protocol")); }
    if hello.request_id != request_id || hello.session_id != session_id { return Err(HttpError::bad_request("INVALID_WALLET_HELLO", "request or session binding does not match")); }
    let wallet_key = decode_frame(&hello.wallet_public_key)?;
    let nonce = decode_frame(&hello.nonce)?;
    if wallet_key.len() != 32 || nonce.len() != 32 { return Err(HttpError::bad_request("INVALID_WALLET_HELLO", "wallet key and nonce must be 32 bytes")); }
    parse_public_key(&hello.channel.user_public_key, "user_public_key")?;
    parse_bytes32(&hello.channel.user_puzzle_hash, "user_puzzle_hash")?;
    Ok(())
}

fn funding_request_signature_hash(request: &FundingRequestFinal) -> Bytes32 {
    let fields: Vec<Vec<u8>> = vec![
        b"XHUB_FUNDING_REQUEST_V1".to_vec(), request.version.to_be_bytes().to_vec(), text_bytes(&request.request_id), text_bytes(&request.session_id), request.created_at.to_be_bytes().to_vec(), request.expires_at.to_be_bytes().to_vec(), text_bytes(request.network), text_bytes(&request.hub_key_id), text_bytes(request.funding.asset_id), request.funding.amount_mojos.parse::<u64>().expect("server generated amount").to_be_bytes().to_vec(), request.funding.required_confirmations.to_be_bytes().to_vec(), request.channel.protocol_version.to_be_bytes().to_vec(),
        hex::decode(&request.channel.hub_public_key).expect("server generated hub key"), hex::decode(&request.channel.user_public_key).expect("validated wallet key"), hex::decode(&request.channel.user_puzzle_hash).expect("validated wallet puzzle hash"), request.channel.claim_before_height.to_be_bytes().to_vec(), request.channel.refund_height.to_be_bytes().to_vec(), hex::decode(&request.channel.funding_puzzle_hash).expect("server generated puzzle hash"), hex::decode(&request.channel.channel_terms_hash).expect("server generated terms hash"), URL_SAFE_NO_PAD.decode(&request.wallet_public_key).expect("validated wallet ephemeral key"), URL_SAFE_NO_PAD.decode(&request.wallet_nonce).expect("validated wallet nonce"),
    ];
    crate::hash_parts(&fields.iter().map(Vec::as_slice).collect::<Vec<_>>())
}

fn text_bytes(value: &str) -> Vec<u8> {
    let mut bytes = u32::try_from(value.len()).expect("protocol text length").to_be_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::{FundingCoinCandidate, funding_status};

    fn candidate(id: &str, confirmed_height: Option<u32>) -> FundingCoinCandidate {
        FundingCoinCandidate {
            funding_coin_id: id.to_string(),
            amount: 10,
            confirmed_height,
        }
    }

    #[test]
    fn funding_status_requires_one_confirmed_coin() {
        assert_eq!(
            funding_status(&[]),
            ("WAITING_FOR_FUNDING".to_string(), None, None)
        );
        assert_eq!(
            funding_status(&[candidate("pending", None)]),
            ("PENDING_CONFIRMATION".to_string(), None, None)
        );
        assert_eq!(
            funding_status(&[candidate("confirmed", Some(42))]),
            (
                "FUNDING_CONFIRMED".to_string(),
                Some("confirmed".to_string()),
                Some(42)
            )
        );
        assert_eq!(
            funding_status(&[candidate("first", Some(42)), candidate("second", Some(43)),]),
            ("AMBIGUOUS_FUNDING".to_string(), None, None)
        );
    }
}
