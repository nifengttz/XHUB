use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request, StatusCode},
    middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chia_bls::SecretKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalDecode, CanonicalEncode, ChannelTerms, LedgerEntry, PROTOCOL_VERSION,
    RecoveryPackage, ReservationStatus, SignatureBytes, sha256_parts,
};

use crate::{
    ChainChannelRegistration, ChainStateProvider, FUNDING_CONFIRMATION_BLOCKS_TEST, HubError,
    HubStore, RecoveryDelivery, RecoveryDeliveryStatus, ReservationLookup, ReservationOutcome,
    ReservationRequest,
};

pub const API_PREFIX: &str = "/api/v3.6";
pub const PROTOCOL_VERSION_TEXT: &str = "0x0360";
pub const PROTOCOL_VERSION_HEADER: &str = "x-xhub-protocol-version";

pub trait RecoveryPackageTransport: Send + Sync {
    fn recipient_ids(&self) -> Vec<String> {
        Vec::new()
    }

    fn deliver(
        &self,
        recipient_id: &str,
        recipient_kind: &str,
        idempotency_key: &str,
        package: &RecoveryPackage,
    ) -> std::result::Result<(), DeliveryTransportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTransportError {
    pub retryable: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct ApiState {
    store: Arc<Mutex<HubStore>>,
    chain: Arc<dyn ChainStateProvider>,
    hub_secret_key: Arc<SecretKey>,
    delivery_transport: Arc<dyn RecoveryPackageTransport>,
}

impl ApiState {
    pub fn new(
        store: HubStore,
        chain: Arc<dyn ChainStateProvider>,
        hub_secret_key: SecretKey,
        delivery_transport: Arc<dyn RecoveryPackageTransport>,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            chain,
            hub_secret_key: Arc::new(hub_secret_key),
            delivery_transport,
        }
    }

    pub fn shared_store(&self) -> Arc<Mutex<HubStore>> {
        Arc::clone(&self.store)
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/v3.6/health", get(health))
        .route("/api/v3.6/funding-coins", post(register_funding_coin))
        .route("/api/v3.6/reservations", post(create_reservation))
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/reservations/{reservation_nonce}",
            get(reservation_status),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/latest",
            get(latest_recovery_package),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}",
            get(recovery_package),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}/deliveries",
            post(deliver_recovery_package).get(recovery_deliveries),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}/watchtower-quorum-deliveries",
            post(deliver_recovery_package_quorum),
        )
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({
        "protocol_version": PROTOCOL_VERSION_TEXT,
        "service": "hub",
        "status": "READY"
    }))
}

pub fn authenticated_router(state: ApiState, bearer_token: String) -> Router {
    router(state).route_layer(middleware::from_fn_with_state(
        AuthState::new(bearer_token),
        require_bearer_token,
    ))
}

#[derive(Clone)]
struct AuthState {
    token_hash: Bytes32,
}

impl AuthState {
    fn new(token: String) -> Self {
        Self {
            token_hash: sha256_parts(&[b"XHUB_API_BEARER_V3_6", token.as_bytes()]),
        }
    }
}

async fn require_bearer_token(
    State(auth): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let authorized = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|token| sha256_parts(&[b"XHUB_API_BEARER_V3_6", token.as_bytes()]))
        .is_some_and(|hash| constant_time_eq(&hash, &auth.token_hash));
    if authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                protocol_version: PROTOCOL_VERSION_TEXT,
                result_class: "REJECTED",
                client_action: "STOP",
                code: "UNAUTHORIZED",
                ledger_written: Some(false),
                message: "a valid Bearer token is required".into(),
            }),
        )
            .into_response()
    }
}

fn constant_time_eq(left: &Bytes32, right: &Bytes32) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservationRequestDto {
    protocol_version: String,
    request_id: String,
    funding_coin_id: String,
    merchant_puzzle_hash: String,
    merchant_receipt_public_key: String,
    amount: String,
    reservation_nonce: String,
    user_authorization_signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FundingRegistrationRequest {
    protocol_version: String,
    funding_coin_id: String,
    funding_puzzle_reveal_hex: String,
    channel_terms_canonical_hex: String,
}

#[derive(Debug, Serialize)]
struct FundingRegistrationResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    channel_terms_hash: String,
    funding_birth_height: u64,
    acceptance_cutoff_height: u64,
    scheduled_close_height: u64,
    confirmation_blocks: u64,
    chain_state: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryRequestDto {
    protocol_version: String,
    recipient_id: String,
    recipient_kind: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuorumDeliveryRequestDto {
    protocol_version: String,
    idempotency_key: String,
}

#[derive(Debug, Serialize)]
struct ReservationResponse {
    protocol_version: &'static str,
    result_class: &'static str,
    client_action: &'static str,
    status: &'static str,
    ledger_written: Option<bool>,
    request_id: Option<String>,
    funding_coin_id: String,
    reservation_nonce: String,
    authorization_hash: Option<String>,
    state_sequence: Option<u64>,
    checkpoint_hash: Option<String>,
    observed_peak_height: Option<u64>,
    acceptance_cutoff_height: Option<u64>,
    scheduled_close_height: Option<u64>,
    signed_result_canonical_hex: Option<String>,
    recovery_package_content_hash: Option<String>,
}

#[derive(Debug, Serialize)]
struct RecoveryPackageResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    recovery_package_canonical_hex: String,
}

#[derive(Debug, Serialize)]
struct DeliveryResponse {
    protocol_version: &'static str,
    delivery: DeliveryDto,
}

#[derive(Debug, Serialize)]
struct DeliveryListResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    deliveries: Vec<DeliveryDto>,
}

#[derive(Debug, Serialize)]
struct QuorumDeliveryResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    configured_recipient_count: usize,
    quorum_required: usize,
    delivered_count: usize,
    retryable_failure_count: usize,
    final_failure_count: usize,
    quorum_met: bool,
    deliveries: Vec<DeliveryDto>,
}

#[derive(Debug, Serialize)]
struct DeliveryDto {
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    recipient_id: String,
    recipient_kind: String,
    idempotency_key: String,
    status: &'static str,
    attempt_count: u64,
    last_error: Option<String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    protocol_version: &'static str,
    result_class: &'static str,
    client_action: &'static str,
    code: &'static str,
    ledger_written: Option<bool>,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    http_status: StatusCode,
    result_class: &'static str,
    client_action: &'static str,
    code: &'static str,
    ledger_written: Option<bool>,
    message: String,
}

async fn register_funding_coin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(dto): Json<FundingRegistrationRequest>,
) -> Result<(StatusCode, Json<FundingRegistrationResponse>), ApiError> {
    require_version(&headers, &dto.protocol_version)?;
    let terms_bytes = parse_variable_hex(&dto.channel_terms_canonical_hex, "channel_terms")?;
    let channel_terms = ChannelTerms::from_canonical_bytes(&terms_bytes)
        .map_err(|error| ApiError::rejected("INVALID_CHANNEL_TERMS", error.to_string()))?;
    let registration = ChainChannelRegistration {
        funding_coin_id: parse_hex(&dto.funding_coin_id, "funding_coin_id")?,
        funding_puzzle_reveal: parse_variable_hex(
            &dto.funding_puzzle_reveal_hex,
            "funding_puzzle_reveal",
        )?,
        channel_terms,
    };
    let terms_hash = registration
        .channel_terms
        .hash()
        .map_err(|error| ApiError::rejected("INVALID_CHANNEL_TERMS", error.to_string()))?;
    let store = state.shared_store();
    let chain = Arc::clone(&state.chain);
    let now = unix_now()?;
    let snapshot = tokio::task::spawn_blocking(move || {
        store.blocking_lock().register_channel_from_chain(
            &registration,
            chain.as_ref(),
            FUNDING_CONFIRMATION_BLOCKS_TEST,
            now,
        )
    })
    .await
    .map_err(|error| blocking_worker_error("FUNDING_REGISTRATION_WORKER_FAILED", error))?
    .map_err(map_registration_error)?;
    Ok((
        StatusCode::CREATED,
        Json(FundingRegistrationResponse {
            protocol_version: PROTOCOL_VERSION_TEXT,
            funding_coin_id: hex::encode(snapshot.funding_coin_id),
            channel_terms_hash: hex::encode(terms_hash),
            funding_birth_height: snapshot.funding_birth_height,
            acceptance_cutoff_height: snapshot.acceptance_cutoff_height,
            scheduled_close_height: snapshot.scheduled_close_height,
            confirmation_blocks: FUNDING_CONFIRMATION_BLOCKS_TEST,
            chain_state: chain_state_name(snapshot.chain_state),
        }),
    ))
}

impl ApiError {
    fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            http_status: StatusCode::BAD_REQUEST,
            result_class: "REJECTED",
            client_action: "STOP",
            code,
            ledger_written: Some(false),
            message: message.into(),
        }
    }

    fn unknown(
        http_status: StatusCode,
        code: &'static str,
        action: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            http_status,
            result_class: "UNKNOWN",
            client_action: action,
            code,
            ledger_written: None,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.http_status,
            Json(ErrorBody {
                protocol_version: PROTOCOL_VERSION_TEXT,
                result_class: self.result_class,
                client_action: self.client_action,
                code: self.code,
                ledger_written: self.ledger_written,
                message: self.message,
            }),
        )
            .into_response()
    }
}

async fn create_reservation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(dto): Json<ReservationRequestDto>,
) -> Result<(StatusCode, Json<ReservationResponse>), ApiError> {
    require_version(&headers, &dto.protocol_version)?;
    let request = ReservationRequest {
        request_id: parse_hex(&dto.request_id, "request_id")?,
        funding_coin_id: parse_hex(&dto.funding_coin_id, "funding_coin_id")?,
        ledger_entry: LedgerEntry {
            merchant_puzzle_hash: parse_hex(&dto.merchant_puzzle_hash, "merchant_puzzle_hash")?,
            merchant_receipt_public_key: parse_hex(
                &dto.merchant_receipt_public_key,
                "merchant_receipt_public_key",
            )?,
            amount: parse_amount(&dto.amount)?,
            reservation_nonce: parse_hex(&dto.reservation_nonce, "reservation_nonce")?,
        },
        user_authorization_signature: parse_hex(
            &dto.user_authorization_signature,
            "user_authorization_signature",
        )?,
    };
    let funding_coin_id = request.funding_coin_id;
    let reservation_nonce = request.ledger_entry.reservation_nonce;
    let store = state.shared_store();
    let chain = Arc::clone(&state.chain);
    let hub_secret_key = Arc::clone(&state.hub_secret_key);
    let now = unix_now()?;
    let result = tokio::task::spawn_blocking(move || {
        store.blocking_lock().reserve_with_chain(
            &request,
            chain.as_ref(),
            hub_secret_key.as_ref(),
            now,
        )
    })
    .await
    .map_err(|error| blocking_worker_error("RESERVATION_WORKER_FAILED", error))?
    .map_err(map_reservation_error)?;
    Ok((
        StatusCode::OK,
        Json(reservation_response(
            funding_coin_id,
            reservation_nonce,
            result,
        )?),
    ))
}

async fn reservation_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, reservation_nonce)): Path<(String, String)>,
    Query(query): Query<VersionQuery>,
) -> Result<(StatusCode, Json<ReservationResponse>), ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let reservation_nonce = parse_hex(&reservation_nonce, "reservation_nonce")?;
    match state
        .store
        .lock()
        .await
        .reservation_status(funding_coin_id, reservation_nonce)
    {
        Ok(ReservationLookup::Pending) => Ok((
            StatusCode::ACCEPTED,
            Json(pending_response(funding_coin_id, reservation_nonce)),
        )),
        Ok(ReservationLookup::Completed(outcome)) => Ok((
            StatusCode::OK,
            Json(reservation_response(
                funding_coin_id,
                reservation_nonce,
                *outcome,
            )?),
        )),
        Err(HubError::ReservationNotFound | HubError::ChannelNotFound) => Err(ApiError::unknown(
            StatusCode::NOT_FOUND,
            "UNKNOWN",
            "RETRY_SAME_NONCE",
            "no persisted result is currently available; query again with the same nonce",
        )),
        Err(error) => Err(map_internal_error(error)),
    }
}

async fn latest_recovery_package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(funding_coin_id): Path<String>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<RecoveryPackageResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let package = state
        .store
        .lock()
        .await
        .latest_recovery_package(funding_coin_id)
        .map_err(map_internal_error)?
        .ok_or_else(|| {
            ApiError::unknown(
                StatusCode::NOT_FOUND,
                "RECOVERY_PACKAGE_NOT_FOUND",
                "PAUSE_AND_QUERY",
                "the channel has no signed recovery package",
            )
        })?;
    Ok(Json(package_response(package)?))
}

async fn recovery_package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence)): Path<(String, u64)>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<RecoveryPackageResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let package = state
        .store
        .lock()
        .await
        .recovery_package(funding_coin_id, state_sequence)
        .map_err(map_package_error)?;
    Ok(Json(package_response(package)?))
}

async fn deliver_recovery_package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence)): Path<(String, u64)>,
    Json(dto): Json<DeliveryRequestDto>,
) -> Result<(StatusCode, Json<DeliveryResponse>), ApiError> {
    require_version(&headers, &dto.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let recipient_kind = dto.recipient_kind.to_ascii_uppercase();
    let (delivery, package) = state
        .store
        .lock()
        .await
        .begin_recovery_delivery(
            funding_coin_id,
            state_sequence,
            &dto.recipient_id,
            &recipient_kind,
            &dto.idempotency_key,
            unix_now()?,
        )
        .map_err(map_delivery_error)?;
    if delivery.status != RecoveryDeliveryStatus::Pending {
        return Ok((StatusCode::OK, Json(delivery_response(delivery))));
    }

    let transport = Arc::clone(&state.delivery_transport);
    let transport_recipient_id = dto.recipient_id.clone();
    let transport_idempotency_key = dto.idempotency_key.clone();
    let transport_recipient_kind = recipient_kind.clone();
    let transport_result = tokio::task::spawn_blocking(move || {
        transport.deliver(
            &transport_recipient_id,
            &transport_recipient_kind,
            &transport_idempotency_key,
            &package,
        )
    })
    .await
    .map_err(|error| {
        ApiError::unknown(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DELIVERY_WORKER_FAILED",
            "PAUSE_AND_QUERY",
            error.to_string(),
        )
    })?;
    let (status, last_error) = match transport_result {
        Ok(()) => (RecoveryDeliveryStatus::Delivered, None),
        Err(error) if error.retryable => {
            (RecoveryDeliveryStatus::FailedRetryable, Some(error.message))
        }
        Err(error) => (RecoveryDeliveryStatus::FailedFinal, Some(error.message)),
    };
    let delivery = state
        .store
        .lock()
        .await
        .finish_recovery_delivery(
            funding_coin_id,
            state_sequence,
            &dto.recipient_id,
            &dto.idempotency_key,
            status,
            last_error.as_deref(),
            unix_now()?,
        )
        .map_err(map_delivery_error)?;
    let http_status = match delivery.status {
        RecoveryDeliveryStatus::Delivered => StatusCode::OK,
        RecoveryDeliveryStatus::FailedRetryable => StatusCode::SERVICE_UNAVAILABLE,
        RecoveryDeliveryStatus::FailedFinal => StatusCode::UNPROCESSABLE_ENTITY,
        RecoveryDeliveryStatus::Pending => StatusCode::ACCEPTED,
    };
    Ok((http_status, Json(delivery_response(delivery))))
}

async fn recovery_deliveries(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence)): Path<(String, u64)>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<DeliveryListResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let deliveries = state
        .store
        .lock()
        .await
        .recovery_deliveries(funding_coin_id, state_sequence)
        .map_err(map_package_error)?;
    Ok(Json(DeliveryListResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        funding_coin_id: hex::encode(funding_coin_id),
        state_sequence,
        deliveries: deliveries.into_iter().map(delivery_dto).collect(),
    }))
}

async fn deliver_recovery_package_quorum(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence)): Path<(String, u64)>,
    Json(dto): Json<QuorumDeliveryRequestDto>,
) -> Result<(StatusCode, Json<QuorumDeliveryResponse>), ApiError> {
    const QUORUM_REQUIRED: usize = 2;

    require_version(&headers, &dto.protocol_version)?;
    let funding_coin_id = parse_hex(&funding_coin_id, "funding_coin_id")?;
    let recipient_ids = state.delivery_transport.recipient_ids();
    if recipient_ids.len() != 3 {
        return Err(ApiError::unknown(
            StatusCode::SERVICE_UNAVAILABLE,
            "WATCHTOWER_QUORUM_NOT_CONFIGURED",
            "PAUSE_AND_QUERY",
            "exactly three Watchtower recipients are required",
        ));
    }

    let mut deliveries = Vec::with_capacity(recipient_ids.len());
    let mut workers = Vec::new();
    for recipient_id in recipient_ids {
        let (delivery, package) = state
            .store
            .lock()
            .await
            .begin_recovery_delivery(
                funding_coin_id,
                state_sequence,
                &recipient_id,
                "WATCHTOWER",
                &dto.idempotency_key,
                unix_now()?,
            )
            .map_err(map_delivery_error)?;
        if delivery.status != RecoveryDeliveryStatus::Pending {
            deliveries.push(delivery);
            continue;
        }

        let transport = Arc::clone(&state.delivery_transport);
        let idempotency_key = dto.idempotency_key.clone();
        workers.push((
            recipient_id.clone(),
            tokio::task::spawn_blocking(move || {
                transport.deliver(&recipient_id, "WATCHTOWER", &idempotency_key, &package)
            }),
        ));
    }

    for (recipient_id, worker) in workers {
        let result = worker.await.map_err(|error| {
            ApiError::unknown(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DELIVERY_WORKER_FAILED",
                "PAUSE_AND_QUERY",
                error.to_string(),
            )
        })?;
        let (status, last_error) = match result {
            Ok(()) => (RecoveryDeliveryStatus::Delivered, None),
            Err(error) if error.retryable => {
                (RecoveryDeliveryStatus::FailedRetryable, Some(error.message))
            }
            Err(error) => (RecoveryDeliveryStatus::FailedFinal, Some(error.message)),
        };
        deliveries.push(
            state
                .store
                .lock()
                .await
                .finish_recovery_delivery(
                    funding_coin_id,
                    state_sequence,
                    &recipient_id,
                    &dto.idempotency_key,
                    status,
                    last_error.as_deref(),
                    unix_now()?,
                )
                .map_err(map_delivery_error)?,
        );
    }

    deliveries.sort_by(|left, right| left.recipient_id.cmp(&right.recipient_id));
    let delivered_count = deliveries
        .iter()
        .filter(|delivery| delivery.status == RecoveryDeliveryStatus::Delivered)
        .count();
    let retryable_failure_count = deliveries
        .iter()
        .filter(|delivery| delivery.status == RecoveryDeliveryStatus::FailedRetryable)
        .count();
    let final_failure_count = deliveries
        .iter()
        .filter(|delivery| delivery.status == RecoveryDeliveryStatus::FailedFinal)
        .count();
    let quorum_met = delivered_count >= QUORUM_REQUIRED;
    let http_status = if quorum_met {
        StatusCode::OK
    } else if delivered_count + retryable_failure_count >= QUORUM_REQUIRED {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    Ok((
        http_status,
        Json(QuorumDeliveryResponse {
            protocol_version: PROTOCOL_VERSION_TEXT,
            funding_coin_id: hex::encode(funding_coin_id),
            state_sequence,
            configured_recipient_count: deliveries.len(),
            quorum_required: QUORUM_REQUIRED,
            delivered_count,
            retryable_failure_count,
            final_failure_count,
            quorum_met,
            deliveries: deliveries.into_iter().map(delivery_dto).collect(),
        }),
    ))
}

fn require_version(headers: &HeaderMap, body_or_query_version: &str) -> Result<(), ApiError> {
    let header = headers
        .get(PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::rejected(
                "PROTOCOL_VERSION_REQUIRED",
                format!("{PROTOCOL_VERSION_HEADER} must be {PROTOCOL_VERSION_TEXT}"),
            )
        })?;
    if header != PROTOCOL_VERSION_TEXT || body_or_query_version != PROTOCOL_VERSION_TEXT {
        return Err(ApiError::rejected(
            "PROTOCOL_VERSION_MISMATCH",
            format!("only protocol version {PROTOCOL_VERSION_TEXT} is accepted"),
        ));
    }
    debug_assert_eq!(PROTOCOL_VERSION, 0x0360);
    Ok(())
}

fn blocking_worker_error(code: &'static str, error: tokio::task::JoinError) -> ApiError {
    ApiError::unknown(
        StatusCode::INTERNAL_SERVER_ERROR,
        code,
        "PAUSE_AND_QUERY",
        error.to_string(),
    )
}

fn reservation_response(
    funding_coin_id: Bytes32,
    reservation_nonce: Bytes32,
    outcome: ReservationOutcome,
) -> Result<ReservationResponse, ApiError> {
    let recovery_package_content_hash = outcome
        .recovery_package
        .as_ref()
        .map(RecoveryPackage::content_hash)
        .transpose()
        .map_err(|error| map_internal_error(HubError::Protocol(error)))?;
    let result = &outcome.signed_result.result;
    let (result_class, client_action) = status_semantics(result.status);
    Ok(ReservationResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        result_class,
        client_action,
        status: status_name(result.status),
        ledger_written: (result_class != "UNKNOWN").then_some(result.ledger_written),
        request_id: Some(hex::encode(result.request_id)),
        funding_coin_id: hex::encode(funding_coin_id),
        reservation_nonce: hex::encode(reservation_nonce),
        authorization_hash: Some(hex::encode(result.authorization_hash)),
        state_sequence: result.state_sequence,
        checkpoint_hash: result.checkpoint_hash.map(hex::encode),
        observed_peak_height: Some(result.observed_peak_height),
        acceptance_cutoff_height: Some(result.acceptance_cutoff_height),
        scheduled_close_height: Some(result.scheduled_close_height),
        signed_result_canonical_hex: Some(hex::encode(outcome.signed_result.canonical_bytes())),
        recovery_package_content_hash: recovery_package_content_hash.map(hex::encode),
    })
}

fn pending_response(funding_coin_id: Bytes32, reservation_nonce: Bytes32) -> ReservationResponse {
    ReservationResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        result_class: "UNKNOWN",
        client_action: "RETRY_SAME_NONCE",
        status: "PENDING",
        ledger_written: None,
        request_id: None,
        funding_coin_id: hex::encode(funding_coin_id),
        reservation_nonce: hex::encode(reservation_nonce),
        authorization_hash: None,
        state_sequence: None,
        checkpoint_hash: None,
        observed_peak_height: None,
        acceptance_cutoff_height: None,
        scheduled_close_height: None,
        signed_result_canonical_hex: None,
        recovery_package_content_hash: None,
    }
}

fn status_semantics(status: ReservationStatus) -> (&'static str, &'static str) {
    match status {
        ReservationStatus::Signed | ReservationStatus::Delivered => ("SUCCESS", "ACCEPT"),
        ReservationStatus::RejectedFreezing
        | ReservationStatus::RejectedCloseable
        | ReservationStatus::InvalidAuthorization
        | ReservationStatus::InsufficientRemainder
        | ReservationStatus::NonceConflict
        | ReservationStatus::LedgerFull
        | ReservationStatus::ChannelClosing
        | ReservationStatus::ChannelFinalized => ("REJECTED", "STOP"),
        ReservationStatus::Pending
        | ReservationStatus::Unknown
        | ReservationStatus::RpcUnavailable
        | ReservationStatus::InternalError => ("UNKNOWN", "RETRY_SAME_NONCE"),
        ReservationStatus::NodeNotSynced
        | ReservationStatus::ChainStateUncertain
        | ReservationStatus::ChannelReorgPending => ("UNKNOWN", "PAUSE_AND_QUERY"),
    }
}

fn status_name(status: ReservationStatus) -> &'static str {
    match status {
        ReservationStatus::Signed => "SIGNED",
        ReservationStatus::Delivered => "DELIVERED",
        ReservationStatus::Pending => "PENDING",
        ReservationStatus::Unknown => "UNKNOWN",
        ReservationStatus::RejectedFreezing => "REJECTED_FREEZING",
        ReservationStatus::RejectedCloseable => "REJECTED_CLOSEABLE",
        ReservationStatus::InvalidAuthorization => "INVALID_AUTHORIZATION",
        ReservationStatus::InsufficientRemainder => "INSUFFICIENT_REMAINDER",
        ReservationStatus::NonceConflict => "NONCE_CONFLICT",
        ReservationStatus::LedgerFull => "LEDGER_FULL",
        ReservationStatus::ChannelClosing => "CHANNEL_CLOSING",
        ReservationStatus::ChannelFinalized => "CHANNEL_FINALIZED",
        ReservationStatus::NodeNotSynced => "NODE_NOT_SYNCED",
        ReservationStatus::RpcUnavailable => "RPC_UNAVAILABLE",
        ReservationStatus::ChainStateUncertain => "CHAIN_STATE_UNCERTAIN",
        ReservationStatus::ChannelReorgPending => "CHANNEL_REORG_PENDING",
        ReservationStatus::InternalError => "INTERNAL_ERROR",
    }
}

fn package_response(package: RecoveryPackage) -> Result<RecoveryPackageResponse, ApiError> {
    let state_sequence = package.official_state.checkpoint.state_sequence;
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| map_internal_error(HubError::Protocol(error)))?;
    let content_hash = package
        .content_hash()
        .map_err(|error| map_internal_error(HubError::Protocol(error)))?;
    Ok(RecoveryPackageResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        funding_coin_id: hex::encode(package.funding_coin_id),
        state_sequence,
        checkpoint_hash: hex::encode(checkpoint_hash),
        recovery_package_content_hash: hex::encode(content_hash),
        recovery_package_canonical_hex: hex::encode(package.canonical_bytes()),
    })
}

fn delivery_response(delivery: RecoveryDelivery) -> DeliveryResponse {
    DeliveryResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        delivery: delivery_dto(delivery),
    }
}

fn delivery_dto(delivery: RecoveryDelivery) -> DeliveryDto {
    DeliveryDto {
        funding_coin_id: hex::encode(delivery.funding_coin_id),
        state_sequence: delivery.state_sequence,
        checkpoint_hash: hex::encode(delivery.checkpoint_hash),
        recovery_package_content_hash: hex::encode(delivery.recovery_package_content_hash),
        recipient_id: delivery.recipient_id,
        recipient_kind: delivery.recipient_kind,
        idempotency_key: delivery.idempotency_key,
        status: delivery.status.as_str(),
        attempt_count: delivery.attempt_count,
        last_error: delivery.last_error,
        created_at: delivery.created_at,
        updated_at: delivery.updated_at,
    }
}

fn parse_hex<const N: usize>(value: &str, field: &'static str) -> Result<[u8; N], ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != N * 2 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(ApiError::rejected(
            "INVALID_ENCODING",
            format!("{field} must encode exactly {N} bytes as hex"),
        ));
    }
    let bytes = hex::decode(value).map_err(|_| {
        ApiError::rejected("INVALID_ENCODING", format!("{field} contains invalid hex"))
    })?;
    bytes.try_into().map_err(|_| {
        ApiError::rejected(
            "INVALID_ENCODING",
            format!("{field} must encode exactly {N} bytes"),
        )
    })
}

fn parse_variable_hex(value: &str, field: &'static str) -> Result<Vec<u8>, ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || value.bytes().any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err(ApiError::rejected(
            "INVALID_ENCODING",
            format!("{field} must be non-empty complete hex bytes"),
        ));
    }
    hex::decode(value).map_err(|_| {
        ApiError::rejected("INVALID_ENCODING", format!("{field} contains invalid hex"))
    })
}

fn parse_amount(value: &str) -> Result<u64, ApiError> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return Err(ApiError::rejected(
            "INVALID_AMOUNT",
            "amount must be a canonical unsigned decimal string",
        ));
    }
    let amount = value.parse::<u64>().map_err(|_| {
        ApiError::rejected(
            "INVALID_AMOUNT",
            "amount must be an unsigned decimal string",
        )
    })?;
    if amount == 0 || amount > xhub_protocol_v3_6::MAX_PROTOCOL_U64 {
        return Err(ApiError::rejected(
            "INVALID_AMOUNT",
            "amount is out of range",
        ));
    }
    Ok(amount)
}

fn map_reservation_error(error: HubError) -> ApiError {
    match error {
        HubError::NonceConflict => ApiError::rejected(
            "NONCE_CONFLICT",
            "the original nonce is bound to different authorization content",
        ),
        HubError::PendingTransition => ApiError::unknown(
            StatusCode::CONFLICT,
            "PENDING",
            "RETRY_SAME_NONCE",
            "a durable transition is pending; query with the same nonce",
        ),
        HubError::Invalid(message) => ApiError::rejected("INVALID_REQUEST", message),
        HubError::Protocol(error) => ApiError::rejected("INVALID_REQUEST", error.to_string()),
        other => map_internal_error(other),
    }
}

fn map_registration_error(error: HubError) -> ApiError {
    match error {
        HubError::ChannelConflict => ApiError::rejected(
            "CHANNEL_CONFLICT",
            "Funding Coin is bound to different terms",
        ),
        HubError::Invalid(message) => ApiError::rejected("INVALID_FUNDING_COIN", message),
        HubError::Protocol(error) => ApiError::rejected("INVALID_CHANNEL_TERMS", error.to_string()),
        HubError::Chain(error) => ApiError::unknown(
            StatusCode::SERVICE_UNAVAILABLE,
            "CHAIN_RPC_UNAVAILABLE",
            "PAUSE_AND_QUERY",
            error.to_string(),
        ),
        other => map_internal_error(other),
    }
}

fn chain_state_name(state: crate::ChannelChainState) -> &'static str {
    match state {
        crate::ChannelChainState::Unconfirmed => "UNCONFIRMED",
        crate::ChannelChainState::Active => "ACTIVE",
        crate::ChannelChainState::NodeNotSynced => "NODE_NOT_SYNCED",
        crate::ChannelChainState::RpcUnavailable => "RPC_UNAVAILABLE",
        crate::ChannelChainState::ChainStateUncertain => "CHAIN_STATE_UNCERTAIN",
        crate::ChannelChainState::ReorgPending => "REORG_PENDING",
        crate::ChannelChainState::Closing => "CLOSING",
    }
}

fn map_package_error(error: HubError) -> ApiError {
    match error {
        HubError::ReservationNotFound | HubError::ChannelNotFound => ApiError::unknown(
            StatusCode::NOT_FOUND,
            "RECOVERY_PACKAGE_NOT_FOUND",
            "PAUSE_AND_QUERY",
            "the requested recovery package is not available",
        ),
        other => map_internal_error(other),
    }
}

fn map_delivery_error(error: HubError) -> ApiError {
    match error {
        HubError::NonceConflict => ApiError::rejected(
            "DELIVERY_CONFLICT",
            "the idempotency key is bound to different delivery content",
        ),
        HubError::Invalid(message) => ApiError::rejected("INVALID_DELIVERY", message),
        other => map_package_error(other),
    }
}

fn map_internal_error(error: HubError) -> ApiError {
    ApiError::unknown(
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        "RETRY_SAME_NONCE",
        format!("operation outcome is uncertain: {error}"),
    )
}

fn unix_now() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            ApiError::unknown(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "RETRY_SAME_NONCE",
                format!("system clock is before the Unix epoch: {error}"),
            )
        })
}

#[allow(dead_code)]
fn _fixed_signature_size(_: SignatureBytes) {}
