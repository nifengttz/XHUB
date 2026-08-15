use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{FundingDraft, FundingDraftStore, FundingTermsInput, WalletError};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

#[derive(Clone, Default)]
pub struct ApiState {
    drafts: Arc<Mutex<FundingDraftStore>>,
}

pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/v3.6/health", get(health))
        .route("/api/v3.6/funding-drafts", post(prepare))
        .route("/api/v3.6/funding-drafts/{draft_id}/confirm", post(confirm))
        .route("/api/v3.6/funding-drafts/{draft_id}", get(get_draft))
        .with_state(ApiState::default())
}

async fn health() -> Json<Value> {
    Json(json!({
        "protocol_version": "0x0360",
        "service": "wallet",
        "status": "READY"
    }))
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
