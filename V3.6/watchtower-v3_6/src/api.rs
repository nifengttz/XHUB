use std::sync::Arc;

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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Mutex;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalDecode, CanonicalEncode, DeliveryConfirmation, PROTOCOL_VERSION,
    RecoveryPackage, sha256_parts,
};

use crate::audit::{ExecutionAuditHead, ExecutionAuditVerification};
use crate::authorization::{ExecutionAuthorization, SimulatedSubmissionReceipt};
use crate::backup::{BackupRestoreDrill, BackupRetentionPolicy, DatabaseBackupManifest};
use crate::{
    CustodyAttestation, GreenlightStatus, ProductionGreenlightStatus, SignedCustodyAttestation,
    SignedDeliveryConfirmation, SingleVpsTestGreenlightStatus, WatchtowerError, WatchtowerStore,
};

pub const API_PREFIX: &str = "/api/v3.6";
pub const PROTOCOL_VERSION_TEXT: &str = "0x0360";
pub const PROTOCOL_VERSION_HEADER: &str = "x-xhub-protocol-version";

#[derive(Clone)]
pub struct ApiState {
    store: Arc<Mutex<WatchtowerStore>>,
}

impl ApiState {
    pub fn new(store: WatchtowerStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    pub fn shared_store(&self) -> Arc<Mutex<WatchtowerStore>> {
        Arc::clone(&self.store)
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/v3.6/health", get(health))
        .route("/api/v3.6/recovery-packages", post(accept_package))
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/latest",
            get(latest_package),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}",
            get(package),
        )
        .route(
            "/api/v3.6/delivery-confirmations",
            post(record_confirmation),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/greenlight",
            get(greenlight),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/custody-attestation",
            get(custody_attestation),
        )
        .route(
            "/api/v3.6/custody-attestations",
            post(record_custody_attestation),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/production-greenlight",
            get(production_greenlight),
        )
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/single-vps-test-greenlight",
            get(single_vps_test_greenlight),
        )
        .route(
            "/api/v3.6/execution-manifests/{manifest_id}/authorization",
            post(issue_execution_authorization),
        )
        .route(
            "/api/v3.6/execution-authorizations/{authorization_id}",
            get(execution_authorization),
        )
        .route(
            "/api/v3.6/execution-authorizations/{authorization_id}/simulate",
            post(simulate_execution_submission),
        )
        .route(
            "/api/v3.6/execution-authorizations/{authorization_id}/simulated-receipt",
            get(simulated_submission_receipt),
        )
        .route("/api/v3.6/execution-audit", get(execution_audit))
        .route(
            "/api/v3.6/backup-restore-drills/{drill_id}",
            get(backup_restore_drill),
        )
        .route(
            "/api/v3.6/backup-retention-candidates",
            post(backup_retention_candidates),
        )
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({
        "protocol_version": PROTOCOL_VERSION_TEXT,
        "service": "watchtower",
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
                code: "UNAUTHORIZED",
                message: "a valid Bearer token is required".into(),
                accepted: false,
                quarantined: false,
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
struct PackageRequest {
    protocol_version: String,
    recovery_package_canonical_hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmationRequest {
    protocol_version: String,
    signer_id: String,
    failure_domain: String,
    signer_public_key: String,
    delivery_confirmation_canonical_hex: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyAttestationRequest {
    protocol_version: String,
    funding_coin_id: String,
    state_sequence: u64,
    entry_index: u64,
    attester_id: String,
    failure_domain: String,
    attester_public_key: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GreenlightQuery {
    protocol_version: String,
    threshold: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationRequest {
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimulatedSubmissionRequest {
    protocol_version: String,
    submission_nonce: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationQuery {
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionRequest {
    protocol_version: String,
    now: u64,
    keep_latest: usize,
    minimum_age_seconds: u64,
    manifests: Vec<RetentionManifestRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionManifestRequest {
    backup_id: String,
    file_hash: String,
    size_bytes: u64,
    audit_event_count: u64,
    audit_head_hash: String,
    anchor_id: Option<String>,
    created_at: u64,
}

#[derive(Debug, Serialize)]
struct PackageResponse {
    protocol_version: &'static str,
    status: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    entry_count: u64,
    recovery_package_canonical_hex: String,
}

#[derive(Debug, Serialize)]
struct ConfirmationResponse {
    protocol_version: &'static str,
    status: &'static str,
    signer_id: String,
    funding_coin_id: String,
    state_sequence: u64,
    entry_index: u64,
}

#[derive(Debug, Serialize)]
struct GreenlightResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    entry_index: u64,
    authorization_hash: String,
    threshold: u16,
    signer_count: u16,
    failure_domain_count: u16,
    delivered: bool,
}

#[derive(Debug, Serialize)]
struct CustodyAttestationResponse {
    protocol_version: &'static str,
    status: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    entry_index: u64,
    attester_id: Option<String>,
    custody_attestation_canonical_hex: String,
    custody_attestation_hash: String,
}

#[derive(Debug, Serialize)]
struct ProductionGreenlightResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    entry_index: u64,
    authorization_hash: String,
    delivery_confirmation_hash: String,
    merchant_delivered: bool,
    custody_threshold: u16,
    custody_attester_count: u16,
    custody_failure_domain_count: u16,
    production_ready: bool,
}

#[derive(Debug, Serialize)]
struct SingleVpsTestGreenlightResponse {
    protocol_version: &'static str,
    funding_coin_id: String,
    state_sequence: u64,
    checkpoint_hash: String,
    recovery_package_content_hash: String,
    entry_index: u64,
    authorization_hash: String,
    delivery_confirmation_hash: String,
    merchant_delivered: bool,
    custody_threshold: u16,
    custody_attester_count: u16,
    observed_failure_domain_count: u16,
    failure_domain_enforced: bool,
    test_only: bool,
    test_ready: bool,
    production_ready: bool,
}

#[derive(Debug, Serialize)]
struct ExecutionAuthorizationResponse {
    protocol_version: &'static str,
    authorization_id: String,
    manifest_id: String,
    recheck_id: String,
    preparation_id: String,
    closing_coin_id: String,
    funding_coin_id: String,
    fee_coin_id: String,
    report_hash: String,
    bundle_commitment: String,
    approval_set_hash: String,
    peak_height: u64,
    peak_header_hash: String,
    challenge_deadline_height: u64,
    issued_at: u64,
    expires_at: u64,
    status: String,
    invalidation_reason: Option<String>,
    simulated_submission_count: u64,
    last_simulated_at: Option<u64>,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct SimulatedSubmissionReceiptResponse {
    protocol_version: &'static str,
    receipt_id: String,
    authorization_id: String,
    manifest_id: String,
    bundle_commitment: String,
    submission_nonce: String,
    consumed_at: u64,
    status: String,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct ExecutionAuditResponse {
    protocol_version: &'static str,
    event_count: u64,
    head_hash: String,
    valid: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct BackupRestoreDrillResponse {
    protocol_version: &'static str,
    drill_id: String,
    artifact_hash: String,
    backup_id: String,
    started_at: u64,
    completed_at: u64,
    duration_seconds: u64,
    hash_matches: bool,
    size_matches: bool,
    audit_valid: bool,
    anchor_valid: Option<bool>,
    status: String,
    failure_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct BackupRetentionResponse {
    protocol_version: &'static str,
    candidate_backup_ids: Vec<String>,
    deletion_performed: bool,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    protocol_version: &'static str,
    code: &'static str,
    message: String,
    accepted: bool,
    quarantined: bool,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    quarantined: bool,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                protocol_version: PROTOCOL_VERSION_TEXT,
                code: self.code,
                message: self.message,
                accepted: false,
                quarantined: self.quarantined,
            }),
        )
            .into_response()
    }
}

async fn accept_package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<PackageRequest>,
) -> Result<(StatusCode, Json<PackageResponse>), ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let bytes = parse_variable_hex(&request.recovery_package_canonical_hex)?;
    let accepted = state
        .store
        .lock()
        .map_err(lock_error)?
        .accept_package(&bytes, unix_now()?)
        .map_err(package_error)?;
    let package = state
        .store
        .lock()
        .map_err(lock_error)?
        .package(accepted.funding_coin_id, accepted.state_sequence)
        .map_err(internal_error)?;
    Ok((StatusCode::OK, Json(package_response(package)?)))
}

async fn latest_package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(funding_coin_id): Path<String>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<PackageResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let package = state
        .store
        .lock()
        .map_err(lock_error)?
        .latest_package(parse_fixed_hex(&funding_coin_id, "funding_coin_id")?)
        .map_err(package_error)?;
    Ok(Json(package_response(package)?))
}

async fn package(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence)): Path<(String, u64)>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<PackageResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let package = state
        .store
        .lock()
        .map_err(lock_error)?
        .package(
            parse_fixed_hex(&funding_coin_id, "funding_coin_id")?,
            state_sequence,
        )
        .map_err(package_error)?;
    Ok(Json(package_response(package)?))
}

async fn record_confirmation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ConfirmationRequest>,
) -> Result<(StatusCode, Json<ConfirmationResponse>), ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let confirmation_bytes = parse_variable_hex(&request.delivery_confirmation_canonical_hex)?;
    let confirmation = DeliveryConfirmation::from_canonical_bytes(&confirmation_bytes)
        .map_err(|error| rejected("INVALID_CONFIRMATION", error.to_string(), false))?;
    let signed = SignedDeliveryConfirmation {
        confirmation,
        signer_id: request.signer_id,
        failure_domain: request.failure_domain,
        signer_public_key: parse_fixed_hex(&request.signer_public_key, "signer_public_key")?,
        signature: parse_fixed_hex(&request.signature, "signature")?,
    };
    state
        .store
        .lock()
        .map_err(lock_error)?
        .record_confirmation(&signed, unix_now()?)
        .map_err(confirmation_error)?;
    Ok((
        StatusCode::OK,
        Json(ConfirmationResponse {
            protocol_version: PROTOCOL_VERSION_TEXT,
            status: "ACCEPTED",
            signer_id: signed.signer_id,
            funding_coin_id: hex::encode(signed.confirmation.funding_coin_id),
            state_sequence: signed.confirmation.state_sequence,
            entry_index: signed.confirmation.entry_index,
        }),
    ))
}

async fn greenlight(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence, entry_index)): Path<(String, u64, u64)>,
    Query(query): Query<GreenlightQuery>,
) -> Result<Json<GreenlightResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let status = state
        .store
        .lock()
        .map_err(lock_error)?
        .greenlight_status(
            parse_fixed_hex(&funding_coin_id, "funding_coin_id")?,
            state_sequence,
            entry_index,
            query.threshold,
        )
        .map_err(confirmation_error)?;
    Ok(Json(greenlight_response(status)))
}

async fn custody_attestation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence, entry_index)): Path<(String, u64, u64)>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<CustodyAttestationResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let attestation = state
        .store
        .lock()
        .map_err(lock_error)?
        .custody_attestation(
            parse_fixed_hex(&funding_coin_id, "funding_coin_id")?,
            state_sequence,
            entry_index,
        )
        .map_err(custody_error)?;
    Ok(Json(custody_attestation_response(
        attestation,
        "SIGNING_PAYLOAD",
        None,
    )))
}

async fn record_custody_attestation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CustodyAttestationRequest>,
) -> Result<(StatusCode, Json<CustodyAttestationResponse>), ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let funding_coin_id = parse_fixed_hex(&request.funding_coin_id, "funding_coin_id")?;
    let mut store = state.store.lock().map_err(lock_error)?;
    let attestation = store
        .custody_attestation(funding_coin_id, request.state_sequence, request.entry_index)
        .map_err(custody_error)?;
    let signed = SignedCustodyAttestation {
        attestation: attestation.clone(),
        attester_id: request.attester_id,
        failure_domain: request.failure_domain,
        attester_public_key: parse_fixed_hex(&request.attester_public_key, "attester_public_key")?,
        signature: parse_fixed_hex(&request.signature, "signature")?,
    };
    store
        .record_custody_attestation(&signed, unix_now()?)
        .map_err(custody_error)?;
    Ok((
        StatusCode::OK,
        Json(custody_attestation_response(
            attestation,
            "ACCEPTED",
            Some(signed.attester_id),
        )),
    ))
}

async fn production_greenlight(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence, entry_index)): Path<(String, u64, u64)>,
    Query(query): Query<GreenlightQuery>,
) -> Result<Json<ProductionGreenlightResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let status = state
        .store
        .lock()
        .map_err(lock_error)?
        .production_greenlight_status(
            parse_fixed_hex(&funding_coin_id, "funding_coin_id")?,
            state_sequence,
            entry_index,
            query.threshold,
        )
        .map_err(custody_error)?;
    Ok(Json(production_greenlight_response(status)))
}

async fn single_vps_test_greenlight(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((funding_coin_id, state_sequence, entry_index)): Path<(String, u64, u64)>,
    Query(query): Query<GreenlightQuery>,
) -> Result<Json<SingleVpsTestGreenlightResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let status = state
        .store
        .lock()
        .map_err(lock_error)?
        .single_vps_test_greenlight_status(
            parse_fixed_hex(&funding_coin_id, "funding_coin_id")?,
            state_sequence,
            entry_index,
            query.threshold,
        )
        .map_err(custody_error)?;
    Ok(Json(single_vps_test_greenlight_response(status)))
}

async fn issue_execution_authorization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(manifest_id): Path<String>,
    Json(request): Json<AuthorizationRequest>,
) -> Result<(StatusCode, Json<ExecutionAuthorizationResponse>), ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let authorization = state
        .store
        .lock()
        .map_err(lock_error)?
        .issue_execution_authorization(parse_fixed_hex(&manifest_id, "manifest_id")?, unix_now()?)
        .map_err(authorization_error)?;
    Ok((StatusCode::OK, Json(authorization_response(authorization))))
}

async fn execution_authorization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(authorization_id): Path<String>,
    Query(query): Query<AuthorizationQuery>,
) -> Result<Json<ExecutionAuthorizationResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let authorization = state
        .store
        .lock()
        .map_err(lock_error)?
        .execution_authorization(
            parse_fixed_hex(&authorization_id, "authorization_id")?,
            unix_now()?,
        )
        .map_err(authorization_error)?
        .ok_or_else(|| {
            rejected(
                "AUTHORIZATION_NOT_FOUND",
                "execution authorization was not found",
                false,
            )
        })?;
    Ok(Json(authorization_response(authorization)))
}

async fn simulate_execution_submission(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(authorization_id): Path<String>,
    Json(request): Json<SimulatedSubmissionRequest>,
) -> Result<Json<SimulatedSubmissionReceiptResponse>, ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let receipt = state
        .store
        .lock()
        .map_err(lock_error)?
        .simulate_execution_submission(
            parse_fixed_hex(&authorization_id, "authorization_id")?,
            parse_fixed_hex(&request.submission_nonce, "submission_nonce")?,
            unix_now()?,
        )
        .map_err(authorization_error)?;
    Ok(Json(simulated_submission_receipt_response(receipt)))
}

async fn simulated_submission_receipt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(authorization_id): Path<String>,
    Query(query): Query<AuthorizationQuery>,
) -> Result<Json<SimulatedSubmissionReceiptResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let receipt = state
        .store
        .lock()
        .map_err(lock_error)?
        .simulated_submission_receipt(parse_fixed_hex(&authorization_id, "authorization_id")?)
        .map_err(authorization_error)?
        .ok_or_else(|| {
            rejected(
                "SIMULATED_RECEIPT_NOT_FOUND",
                "simulated submission receipt was not found",
                false,
            )
        })?;
    Ok(Json(simulated_submission_receipt_response(receipt)))
}

async fn execution_audit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<AuthorizationQuery>,
) -> Result<Json<ExecutionAuditResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let verification = state
        .store
        .lock()
        .map_err(lock_error)?
        .verify_execution_audit_chain()
        .map_err(authorization_error)?;
    Ok(Json(execution_audit_response(verification)))
}

async fn backup_restore_drill(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(drill_id): Path<String>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<BackupRestoreDrillResponse>, ApiError> {
    require_version(&headers, &query.protocol_version)?;
    let drill = state
        .store
        .lock()
        .map_err(lock_error)?
        .backup_restore_drill(parse_fixed_hex(&drill_id, "drill_id")?)
        .map_err(backup_error)?
        .ok_or_else(|| {
            rejected(
                "BACKUP_DRILL_NOT_FOUND",
                "backup restore drill was not found",
                false,
            )
        })?;
    Ok(Json(backup_restore_drill_response(drill)))
}

async fn backup_retention_candidates(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<RetentionRequest>,
) -> Result<Json<BackupRetentionResponse>, ApiError> {
    require_version(&headers, &request.protocol_version)?;
    let manifests = request
        .manifests
        .into_iter()
        .map(retention_manifest)
        .collect::<Result<Vec<_>, _>>()?;
    let candidates = state
        .store
        .lock()
        .map_err(lock_error)?
        .backup_retention_candidates(
            &manifests,
            BackupRetentionPolicy {
                keep_latest: request.keep_latest,
                minimum_age_seconds: request.minimum_age_seconds,
            },
            request.now,
        )
        .map_err(backup_error)?;
    Ok(Json(BackupRetentionResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        candidate_backup_ids: candidates.into_iter().map(hex::encode).collect(),
        deletion_performed: false,
    }))
}

fn package_response(package: RecoveryPackage) -> Result<PackageResponse, ApiError> {
    let checkpoint_hash = package
        .official_state
        .checkpoint
        .hash(&package.channel_terms)
        .map_err(|error| internal_error(error.into()))?;
    let content_hash = package
        .content_hash()
        .map_err(|error| internal_error(error.into()))?;
    Ok(PackageResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        status: "ACCEPTED",
        funding_coin_id: hex::encode(package.funding_coin_id),
        state_sequence: package.official_state.checkpoint.state_sequence,
        checkpoint_hash: hex::encode(checkpoint_hash),
        recovery_package_content_hash: hex::encode(content_hash),
        entry_count: package.entries.len() as u64,
        recovery_package_canonical_hex: hex::encode(package.canonical_bytes()),
    })
}

fn greenlight_response(status: GreenlightStatus) -> GreenlightResponse {
    GreenlightResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        funding_coin_id: hex::encode(status.funding_coin_id),
        state_sequence: status.state_sequence,
        checkpoint_hash: hex::encode(status.checkpoint_hash),
        recovery_package_content_hash: hex::encode(status.recovery_package_content_hash),
        entry_index: status.entry_index,
        authorization_hash: hex::encode(status.authorization_hash),
        threshold: status.threshold,
        signer_count: status.signer_count,
        failure_domain_count: status.failure_domain_count,
        delivered: status.delivered,
    }
}

fn custody_attestation_response(
    attestation: CustodyAttestation,
    status: &'static str,
    attester_id: Option<String>,
) -> CustodyAttestationResponse {
    CustodyAttestationResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        status,
        funding_coin_id: hex::encode(attestation.funding_coin_id),
        state_sequence: attestation.state_sequence,
        entry_index: attestation.entry_index,
        attester_id,
        custody_attestation_hash: hex::encode(attestation.hash()),
        custody_attestation_canonical_hex: hex::encode(attestation.canonical_bytes()),
    }
}

fn production_greenlight_response(
    status: ProductionGreenlightStatus,
) -> ProductionGreenlightResponse {
    ProductionGreenlightResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        funding_coin_id: hex::encode(status.funding_coin_id),
        state_sequence: status.state_sequence,
        checkpoint_hash: hex::encode(status.checkpoint_hash),
        recovery_package_content_hash: hex::encode(status.recovery_package_content_hash),
        entry_index: status.entry_index,
        authorization_hash: hex::encode(status.authorization_hash),
        delivery_confirmation_hash: hex::encode(status.delivery_confirmation_hash),
        merchant_delivered: status.merchant_delivered,
        custody_threshold: status.custody_threshold,
        custody_attester_count: status.custody_attester_count,
        custody_failure_domain_count: status.custody_failure_domain_count,
        production_ready: status.production_ready,
    }
}

fn single_vps_test_greenlight_response(
    status: SingleVpsTestGreenlightStatus,
) -> SingleVpsTestGreenlightResponse {
    SingleVpsTestGreenlightResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        funding_coin_id: hex::encode(status.funding_coin_id),
        state_sequence: status.state_sequence,
        checkpoint_hash: hex::encode(status.checkpoint_hash),
        recovery_package_content_hash: hex::encode(status.recovery_package_content_hash),
        entry_index: status.entry_index,
        authorization_hash: hex::encode(status.authorization_hash),
        delivery_confirmation_hash: hex::encode(status.delivery_confirmation_hash),
        merchant_delivered: status.merchant_delivered,
        custody_threshold: status.custody_threshold,
        custody_attester_count: status.custody_attester_count,
        observed_failure_domain_count: status.observed_failure_domain_count,
        failure_domain_enforced: false,
        test_only: true,
        test_ready: status.test_ready,
        production_ready: false,
    }
}

fn authorization_response(authorization: ExecutionAuthorization) -> ExecutionAuthorizationResponse {
    ExecutionAuthorizationResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        authorization_id: hex::encode(authorization.authorization_id),
        manifest_id: hex::encode(authorization.manifest_id),
        recheck_id: hex::encode(authorization.recheck_id),
        preparation_id: hex::encode(authorization.preparation_id),
        closing_coin_id: hex::encode(authorization.closing_coin_id),
        funding_coin_id: hex::encode(authorization.funding_coin_id),
        fee_coin_id: hex::encode(authorization.fee_coin_id),
        report_hash: hex::encode(authorization.report_hash),
        bundle_commitment: hex::encode(authorization.bundle_commitment),
        approval_set_hash: hex::encode(authorization.approval_set_hash),
        peak_height: authorization.peak_height,
        peak_header_hash: hex::encode(authorization.peak_header_hash),
        challenge_deadline_height: authorization.challenge_deadline_height,
        issued_at: authorization.issued_at,
        expires_at: authorization.expires_at,
        status: authorization.status,
        invalidation_reason: authorization.invalidation_reason,
        simulated_submission_count: authorization.simulated_submission_count,
        last_simulated_at: authorization.last_simulated_at,
        broadcast_enabled: authorization.broadcast_enabled,
        broadcast_ready: authorization.broadcast_ready,
        chain_broadcast: authorization.chain_broadcast,
    }
}

fn simulated_submission_receipt_response(
    receipt: SimulatedSubmissionReceipt,
) -> SimulatedSubmissionReceiptResponse {
    SimulatedSubmissionReceiptResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        receipt_id: hex::encode(receipt.receipt_id),
        authorization_id: hex::encode(receipt.authorization_id),
        manifest_id: hex::encode(receipt.manifest_id),
        bundle_commitment: hex::encode(receipt.bundle_commitment),
        submission_nonce: hex::encode(receipt.submission_nonce),
        consumed_at: receipt.consumed_at,
        status: receipt.status,
        broadcast_enabled: receipt.broadcast_enabled,
        broadcast_ready: receipt.broadcast_ready,
        chain_broadcast: receipt.chain_broadcast,
    }
}

fn execution_audit_response(verification: ExecutionAuditVerification) -> ExecutionAuditResponse {
    let ExecutionAuditHead {
        event_count,
        head_hash,
    } = verification.head;
    ExecutionAuditResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        event_count,
        head_hash: hex::encode(head_hash),
        valid: verification.valid,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    }
}

fn backup_restore_drill_response(drill: BackupRestoreDrill) -> BackupRestoreDrillResponse {
    BackupRestoreDrillResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        drill_id: hex::encode(drill.drill_id),
        artifact_hash: hex::encode(drill.artifact_hash),
        backup_id: hex::encode(drill.backup_id),
        started_at: drill.started_at,
        completed_at: drill.completed_at,
        duration_seconds: drill.duration_seconds,
        hash_matches: drill.hash_matches,
        size_matches: drill.size_matches,
        audit_valid: drill.audit_valid,
        anchor_valid: drill.anchor_valid,
        status: drill.status,
        failure_reason: drill.failure_reason,
    }
}

fn retention_manifest(value: RetentionManifestRequest) -> Result<DatabaseBackupManifest, ApiError> {
    Ok(DatabaseBackupManifest {
        backup_id: parse_fixed_hex(&value.backup_id, "backup_id")?,
        file_hash: parse_fixed_hex(&value.file_hash, "file_hash")?,
        size_bytes: value.size_bytes,
        audit_event_count: value.audit_event_count,
        audit_head_hash: parse_fixed_hex(&value.audit_head_hash, "audit_head_hash")?,
        anchor_id: value
            .anchor_id
            .map(|value| parse_fixed_hex(&value, "anchor_id"))
            .transpose()?,
        created_at: value.created_at,
    })
}

fn require_version(headers: &HeaderMap, value: &str) -> Result<(), ApiError> {
    let header = headers
        .get(PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok());
    if header != Some(PROTOCOL_VERSION_TEXT) || value != PROTOCOL_VERSION_TEXT {
        return Err(rejected(
            "PROTOCOL_VERSION_MISMATCH",
            format!("header and payload must both be {PROTOCOL_VERSION_TEXT}"),
            false,
        ));
    }
    debug_assert_eq!(PROTOCOL_VERSION, 0x0360);
    Ok(())
}

fn parse_variable_hex(value: &str) -> Result<Vec<u8>, ApiError> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err(rejected(
            "INVALID_ENCODING",
            "canonical hex must contain complete bytes",
            false,
        ));
    }
    hex::decode(value).map_err(|error| rejected("INVALID_ENCODING", error.to_string(), false))
}

fn parse_fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], ApiError> {
    let bytes = parse_variable_hex(value)?;
    bytes.try_into().map_err(|_| {
        rejected(
            "INVALID_ENCODING",
            format!("{field} must encode exactly {N} bytes"),
            false,
        )
    })
}

fn package_error(error: WatchtowerError) -> ApiError {
    match error {
        WatchtowerError::PackageNotFound => rejected("PACKAGE_NOT_FOUND", error.to_string(), false),
        WatchtowerError::Protocol(_)
        | WatchtowerError::Invalid(_)
        | WatchtowerError::StateConflict
        | WatchtowerError::StalePackage => rejected("INVALID_PACKAGE", error.to_string(), true),
        other => internal_error(other),
    }
}

fn confirmation_error(error: WatchtowerError) -> ApiError {
    match error {
        WatchtowerError::Invalid(_)
        | WatchtowerError::EntryNotFound
        | WatchtowerError::ConfirmationMismatch
        | WatchtowerError::InvalidConfirmationSignature
        | WatchtowerError::ConfirmerConflict
        | WatchtowerError::DuplicateSigner => {
            rejected("INVALID_CONFIRMATION", error.to_string(), false)
        }
        other => internal_error(other),
    }
}

fn custody_error(error: WatchtowerError) -> ApiError {
    match error {
        WatchtowerError::Invalid(_)
        | WatchtowerError::EntryNotFound
        | WatchtowerError::MerchantConfirmationRequired
        | WatchtowerError::CustodyAttestationMismatch
        | WatchtowerError::InvalidCustodyAttestationSignature
        | WatchtowerError::AttesterConflict
        | WatchtowerError::DuplicateAttester => {
            rejected("INVALID_CUSTODY_ATTESTATION", error.to_string(), false)
        }
        WatchtowerError::PackageNotFound => rejected("PACKAGE_NOT_FOUND", error.to_string(), false),
        other => internal_error(other),
    }
}

fn authorization_error(error: WatchtowerError) -> ApiError {
    match error {
        WatchtowerError::Invalid(_) | WatchtowerError::Corrupt(_) => {
            rejected("AUTHORIZATION_NOT_AVAILABLE", error.to_string(), false)
        }
        other => internal_error(other),
    }
}

fn backup_error(error: WatchtowerError) -> ApiError {
    match error {
        WatchtowerError::Invalid(_) | WatchtowerError::Corrupt(_) => {
            rejected("BACKUP_NOT_AVAILABLE", error.to_string(), false)
        }
        other => internal_error(other),
    }
}

fn rejected(code: &'static str, message: impl Into<String>, quarantined: bool) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        code,
        message: message.into(),
        quarantined,
    }
}

fn internal_error(error: WatchtowerError) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "INTERNAL_ERROR",
        message: error.to_string(),
        quarantined: false,
    }
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "INTERNAL_ERROR",
        message: error.to_string(),
        quarantined: false,
    }
}

fn unix_now() -> Result<u64, ApiError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|error| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR",
            message: error.to_string(),
            quarantined: false,
        })
}

#[allow(dead_code)]
fn _fixed_bytes32(_: Bytes32) {}
