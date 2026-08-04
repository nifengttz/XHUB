use std::{net::SocketAddr, path::Path, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chia_bls::{PublicKey, SecretKey};
use chia_protocol::Bytes32;
use chia_sdk_types::MAINNET_CONSTANTS;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    API_PROTOCOL_VERSION, API_SCHEMA_VERSION, ChannelArgs, ChannelState, ChannelStore, ChiaNode,
    ChiaNodeError, ChiaRpcConfig, InvoiceFields, MerchantInvoice, PaymentIntent, PaymentVoucher,
    StateStoreError,
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
}

#[derive(Clone)]
struct ApiState {
    runtime: Arc<Mutex<HubRuntime>>,
    node: Arc<ChiaNode>,
    api_key: Arc<String>,
    hub_public_key: String,
}

struct HubRuntime {
    hub_secret_key: SecretKey,
    store: ChannelStore,
    agg_sig_me_additional_data: Bytes32,
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
}

#[derive(Debug, Deserialize)]
pub struct InvoiceRequest {
    pub request_id: String,
    pub idempotency_key: String,
    pub channel: ChannelRequest,
    pub funding_coin_id: String,
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
struct HealthResponse {
    status: &'static str,
    role: &'static str,
    schema_version: u16,
    protocol_version: u16,
    hub_public_key: String,
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

    let state = ApiState {
        runtime: Arc::new(Mutex::new(HubRuntime {
            hub_secret_key,
            store,
            agg_sig_me_additional_data: additional_data,
        })),
        node,
        api_key: Arc::new(config.api_key),
        hub_public_key,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/invoices", post(issue_invoice))
        .route("/v1/vouchers", post(issue_voucher))
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

async fn health(State(state): State<ApiState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        role: "hub",
        schema_version: API_SCHEMA_VERSION,
        protocol_version: API_PROTOCOL_VERSION,
        hub_public_key: state.hub_public_key,
    })
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
    validate_mainnet(&args)?;
    let funding_coin_id = parse_bytes32(&request.funding_coin_id, "funding_coin_id")?;
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
            .create_channel(invoice.fields.channel_id)
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

async fn issue_voucher(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<VoucherRequest>,
) -> Result<Json<VoucherResponse>, HttpError> {
    authorize(&headers, &state)?;
    validate_request_meta(&request.request_id, &request.idempotency_key)?;
    let current_height = trusted_peak_height(&state).await?;
    let args = request.channel.channel_args()?;
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
