use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    CANARY_DELIVERY_PARTICIPANTS, CANARY_DELIVERY_THRESHOLD, DEFAULT_ACCEPTANCE_BLOCKS,
    DEFAULT_CHALLENGE_BLOCKS, DEFAULT_FREEZE_BLOCKS, FUNDING_CONFIRMATION_BLOCKS_TEST,
    FundingDraft, FundingDraftStore, FundingTermsInput, MAINNET_NETWORK_ID, PROFILE_ID,
    WalletError,
};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");
pub const DEFAULT_HUB_STATE_PUBLIC_KEY_A: &str = "a35388f0b8fa4d11a2dfbe832a609229585dce39c0bcbde87ec621391876fdc58b0b7648dc469fa1aa73e47bffb38553";

#[derive(Debug, Clone)]
pub struct HubGatewayConfig {
    base_url: String,
    bearer_token: String,
}

impl HubGatewayConfig {
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Result<Self, String> {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|error| format!("invalid HUB gateway URL: {error}"))?;
        if parsed.scheme() != "http"
            || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
        {
            return Err("HUB gateway URL must use HTTP on a loopback host".into());
        }
        let bearer_token = bearer_token.into();
        if bearer_token.len() < 32 {
            return Err("HUB gateway token must contain at least 32 characters".into());
        }
        Ok(Self {
            base_url,
            bearer_token,
        })
    }
}

#[derive(Clone)]
struct HubGateway {
    base_url: Arc<str>,
    bearer_token: Arc<str>,
    client: reqwest::Client,
}

impl HubGateway {
    fn new(config: HubGatewayConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("cannot build HUB gateway client: {error}"))?;
        Ok(Self {
            base_url: config.base_url.into(),
            bearer_token: config.bearer_token.into(),
            client,
        })
    }

    async fn forward(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Response, ApiError> {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(self.bearer_token.as_ref())
            .header("x-xhub-protocol-version", "0x0360");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let upstream = request.send().await.map_err(|error| ApiError {
            status: StatusCode::BAD_GATEWAY,
            code: "HUB_GATEWAY_UNAVAILABLE",
            message: format!("HUB request failed: {error}"),
        })?;
        let status = upstream.status();
        let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
        let bytes = upstream.bytes().await.map_err(|error| ApiError {
            status: StatusCode::BAD_GATEWAY,
            code: "HUB_GATEWAY_INVALID_RESPONSE",
            message: format!("cannot read HUB response: {error}"),
        })?;
        let mut response = Response::builder().status(status);
        if let Some(content_type) = content_type {
            response = response.header(header::CONTENT_TYPE, content_type);
        }
        response.body(Body::from(bytes)).map_err(|error| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR",
            message: error.to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MainnetProfile {
    protocol_version: &'static str,
    profile_id: &'static str,
    network: &'static str,
    network_id: &'static str,
    acceptance_blocks: u64,
    freeze_blocks: u64,
    close_delay_blocks: u64,
    challenge_blocks: u64,
    funding_confirmation_blocks: u64,
    delivery_threshold: u16,
    delivery_participants: u16,
    hub_state_public_key_a: String,
    state_rules_hash: String,
    mainnet_approved: bool,
    production_ready: bool,
    hub_gateway_enabled: bool,
}

impl MainnetProfile {
    fn new(hub_state_public_key_a: &str, hub_gateway_enabled: bool) -> Result<Self, String> {
        if hub_state_public_key_a.len() != 96
            || hub_state_public_key_a
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit())
        {
            return Err("HUB A public key must encode exactly 48 bytes as hex".into());
        }
        let modules = xhub_puzzles_v3_6::module_hashes();
        let state_rules_hash = xhub_protocol_v3_6::state_rules_hash(
            &modules.initial_closing,
            &modules.subsequent_closing,
            &modules.merchant_payment,
        );
        Ok(Self {
            protocol_version: "0x0360",
            profile_id: PROFILE_ID,
            network: "mainnet",
            network_id: MAINNET_NETWORK_ID,
            acceptance_blocks: DEFAULT_ACCEPTANCE_BLOCKS,
            freeze_blocks: DEFAULT_FREEZE_BLOCKS,
            close_delay_blocks: DEFAULT_ACCEPTANCE_BLOCKS + DEFAULT_FREEZE_BLOCKS,
            challenge_blocks: DEFAULT_CHALLENGE_BLOCKS,
            funding_confirmation_blocks: FUNDING_CONFIRMATION_BLOCKS_TEST,
            delivery_threshold: CANARY_DELIVERY_THRESHOLD,
            delivery_participants: CANARY_DELIVERY_PARTICIPANTS,
            hub_state_public_key_a: hub_state_public_key_a.to_ascii_lowercase(),
            state_rules_hash: hex::encode(state_rules_hash),
            mainnet_approved: false,
            production_ready: false,
            hub_gateway_enabled,
        })
    }
}

#[derive(Clone)]
pub struct ApiState {
    drafts: Arc<Mutex<FundingDraftStore>>,
    profile: MainnetProfile,
    gateway: Option<HubGateway>,
}

impl ApiState {
    fn new(profile: MainnetProfile, gateway: Option<HubGateway>) -> Self {
        Self {
            drafts: Arc::new(Mutex::new(FundingDraftStore::default())),
            profile,
            gateway,
        }
    }
}

pub fn router() -> Router {
    router_with_options(DEFAULT_HUB_STATE_PUBLIC_KEY_A, None)
        .expect("built-in HUB public key must be valid")
}

pub fn router_with_gateway(
    hub_state_public_key_a: &str,
    gateway: HubGatewayConfig,
) -> Result<Router, String> {
    router_with_options(hub_state_public_key_a, Some(gateway))
}

fn router_with_options(
    hub_state_public_key_a: &str,
    gateway: Option<HubGatewayConfig>,
) -> Result<Router, String> {
    let gateway = gateway.map(HubGateway::new).transpose()?;
    let profile = MainnetProfile::new(hub_state_public_key_a, gateway.is_some())?;
    Ok(Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/v3.6/health", get(health))
        .route("/api/v3.6/config", get(config))
        .route("/api/v3.6/funding-drafts", post(prepare))
        .route("/api/v3.6/funding-drafts/{draft_id}/confirm", post(confirm))
        .route("/api/v3.6/funding-drafts/{draft_id}", get(get_draft))
        .route("/api/v3.6/hub/health", get(hub_health))
        .route("/api/v3.6/hub/reservations", post(hub_reservation))
        .route(
            "/api/v3.6/hub/funding-coins/{funding_coin_id}/reservations/{reservation_nonce}",
            get(hub_reservation_status),
        )
        .with_state(ApiState::new(profile, gateway)))
}

async fn health() -> Json<Value> {
    Json(json!({
        "protocol_version": "0x0360",
        "service": "wallet",
        "status": "READY"
    }))
}

async fn config(State(state): State<ApiState>) -> Json<MainnetProfile> {
    Json(state.profile)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn css() -> impl IntoResponse {
    ([("content-type", "text/css; charset=utf-8")], APP_CSS)
}

async fn js() -> impl IntoResponse {
    ([("content-type", "text/javascript; charset=utf-8")], APP_JS)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareRequest {
    protocol_version: String,
    #[serde(flatten)]
    terms: FundingTermsInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmRequest {
    protocol_version: String,
    channel_terms_hash: String,
    user_confirmed: bool,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    protocol_version: &'static str,
    code: &'static str,
    message: String,
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                protocol_version: "0x0360",
                code: self.code,
                message: self.message,
            }),
        )
            .into_response()
    }
}

async fn prepare(
    State(state): State<ApiState>,
    Json(request): Json<PrepareRequest>,
) -> Result<Json<FundingDraft>, ApiError> {
    require_version(&request.protocol_version)?;
    if request.terms.network_id.to_ascii_lowercase() != MAINNET_NETWORK_ID {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "NETWORK_MISMATCH",
            message: "the public wallet accepts only the configured Chia mainnet network id".into(),
        });
    }
    if request.terms.hub_state_public_key_a.to_ascii_lowercase()
        != state.profile.hub_state_public_key_a
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "HUB_IDENTITY_MISMATCH",
            message: "the HUB A public key does not match the deployed wallet profile".into(),
        });
    }
    if request.terms.state_rules_hash.to_ascii_lowercase() != state.profile.state_rules_hash {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "STATE_RULES_MISMATCH",
            message: "the state rules hash does not match the deployed modules".into(),
        });
    }
    let draft = state
        .drafts
        .lock()
        .map_err(lock_error)?
        .prepare(&request.terms)
        .map_err(wallet_error)?;
    Ok(Json(draft))
}

async fn confirm(
    State(state): State<ApiState>,
    Path(draft_id): Path<String>,
    Json(request): Json<ConfirmRequest>,
) -> Result<Json<FundingDraft>, ApiError> {
    require_version(&request.protocol_version)?;
    if !request.user_confirmed {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "CONFIRMATION_REQUIRED",
            message: "the user must explicitly confirm the immutable terms".into(),
        });
    }
    let draft = state
        .drafts
        .lock()
        .map_err(lock_error)?
        .confirm(&draft_id, &request.channel_terms_hash)
        .map_err(wallet_error)?;
    Ok(Json(draft))
}

async fn get_draft(
    State(state): State<ApiState>,
    Path(draft_id): Path<String>,
) -> Result<Json<FundingDraft>, ApiError> {
    Ok(Json(
        state
            .drafts
            .lock()
            .map_err(lock_error)?
            .get(&draft_id)
            .map_err(wallet_error)?,
    ))
}

async fn hub_health(State(state): State<ApiState>) -> Result<Response, ApiError> {
    gateway(&state)?
        .forward(Method::GET, "/api/v3.6/health", None)
        .await
}

async fn hub_reservation(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    require_body_version(&body)?;
    gateway(&state)?
        .forward(Method::POST, "/api/v3.6/reservations", Some(body))
        .await
}

async fn hub_reservation_status(
    State(state): State<ApiState>,
    Path((funding_coin_id, reservation_nonce)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    require_hex_path(&funding_coin_id, "funding_coin_id")?;
    require_hex_path(&reservation_nonce, "reservation_nonce")?;
    let path = format!(
        "/api/v3.6/funding-coins/{funding_coin_id}/reservations/{reservation_nonce}?protocol_version=0x0360"
    );
    gateway(&state)?.forward(Method::GET, &path, None).await
}

fn gateway(state: &ApiState) -> Result<&HubGateway, ApiError> {
    state.gateway.as_ref().ok_or_else(|| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "HUB_GATEWAY_DISABLED",
        message: "the server-side HUB gateway is not configured".into(),
    })
}

fn require_body_version(body: &Value) -> Result<(), ApiError> {
    match body.get("protocol_version").and_then(Value::as_str) {
        Some(version) => require_version(version),
        None => Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "PROTOCOL_VERSION_MISMATCH",
            message: "protocol_version 0x0360 is required".into(),
        }),
    }
}

fn require_hex_path(value: &str, field: &str) -> Result<(), ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != 64 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "INVALID_PATH_PARAMETER",
            message: format!("{field} must encode exactly 32 bytes as hex"),
        });
    }
    Ok(())
}

fn require_version(version: &str) -> Result<(), ApiError> {
    if version == "0x0360" {
        Ok(())
    } else {
        Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "PROTOCOL_VERSION_MISMATCH",
            message: "only protocol_version 0x0360 is accepted".into(),
        })
    }
}

fn wallet_error(error: WalletError) -> ApiError {
    let (status, code) = match error {
        WalletError::DraftNotFound => (StatusCode::NOT_FOUND, "DRAFT_NOT_FOUND"),
        WalletError::DraftImmutable => (StatusCode::CONFLICT, "DRAFT_IMMUTABLE"),
        WalletError::ConfirmationMismatch => (StatusCode::CONFLICT, "CONFIRMATION_MISMATCH"),
        WalletError::Protocol(_) | WalletError::Invalid(_) => {
            (StatusCode::BAD_REQUEST, "INVALID_TERMS")
        }
    };
    ApiError {
        status,
        code,
        message: error.to_string(),
    }
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "INTERNAL_ERROR",
        message: error.to_string(),
    }
}
