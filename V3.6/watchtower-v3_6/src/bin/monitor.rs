use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use xhub_watchtower_v3_6::{
    WatchtowerStore,
    monitor::{MonitorAction, MonitorDecision},
    rpc::ChiaRpcProvider,
};

const PROTOCOL_VERSION_TEXT: &str = "0x0360";
const EXECUTION_GATE: &str = "DUAL_APPROVAL_AND_FINAL_RECHECK_REQUIRED";

#[derive(Clone)]
struct MonitorHttpState {
    funding_coin_id: String,
    interval_seconds: u64,
    max_staleness_seconds: u64,
    snapshot: Arc<RwLock<Option<MonitorSnapshot>>>,
}

#[derive(Clone)]
struct MonitorSnapshot {
    decision: MonitorDecision,
    updated_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorStatusQuery {
    protocol_version: String,
}

#[derive(Debug, Serialize)]
struct MonitorStatusResponse {
    protocol_version: &'static str,
    service: &'static str,
    status: &'static str,
    funding_coin_id: String,
    rpc_mode: &'static str,
    interval_seconds: u64,
    max_staleness_seconds: u64,
    last_poll_at: Option<u64>,
    age_seconds: Option<u64>,
    fresh: bool,
    action: Option<MonitorAction>,
    alert_level: &'static str,
    operator_attention_required: bool,
    detail: String,
    peak_height: Option<u64>,
    current_state_sequence: Option<u64>,
    latest_state_sequence: Option<u64>,
    challenge_deadline_height: Option<u64>,
    manual_approval_required: bool,
    execution_gate: &'static str,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Watchtower monitor failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(3..=5).contains(&args.len()) {
        return Err(
            "usage: watchtower-monitor-v3-6 WATCHTOWER_DB RPC_URL FUNDING_COIN_ID [INTERVAL_SECONDS] [LISTEN]"
                .into(),
        );
    }
    let funding_coin_id = parse_bytes32(&args[2])?;
    let funding_coin_id_text = hex::encode(funding_coin_id);
    let interval = parse_interval(args.get(3))?;
    let listen = parse_listen(args.get(4))?;
    if listen.is_some() && interval.is_none() {
        return Err("LISTEN requires INTERVAL_SECONDS".into());
    }

    let provider = ChiaRpcProvider::public(&args[1], Duration::from_secs(20))
        .map_err(|error| error.to_string())?;
    let store = WatchtowerStore::open(&args[0]).map_err(|error| error.to_string())?;
    let snapshot = Arc::new(RwLock::new(None));

    let Some(listen) = listen else {
        return poll_loop(store, provider, funding_coin_id, interval, snapshot);
    };
    let interval = interval.expect("LISTEN requires interval");
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| error.to_string())?;
    let http_state = MonitorHttpState {
        funding_coin_id: funding_coin_id_text,
        interval_seconds: interval.as_secs(),
        max_staleness_seconds: interval.as_secs().saturating_mul(3),
        snapshot: Arc::clone(&snapshot),
    };
    thread::spawn(move || {
        if let Err(error) = poll_loop(store, provider, funding_coin_id, Some(interval), snapshot) {
            eprintln!("Watchtower monitor polling failed: {error}");
            std::process::exit(1);
        }
    });
    println!("XHUB Watchtower V3.6 read-only monitor listening on http://{listen}");
    axum::serve(listener, monitor_router(http_state))
        .await
        .map_err(|error| error.to_string())
}

fn poll_loop(
    mut store: WatchtowerStore,
    provider: ChiaRpcProvider,
    funding_coin_id: [u8; 32],
    interval: Option<Duration>,
    snapshot: Arc<RwLock<Option<MonitorSnapshot>>>,
) -> Result<(), String> {
    loop {
        let now = unix_now()?;
        let decision = store
            .poll_chain(&provider, funding_coin_id, now)
            .map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string(&decision).map_err(|error| error.to_string())?
        );
        *snapshot
            .write()
            .map_err(|error| format!("monitor snapshot lock poisoned: {error}"))? =
            Some(MonitorSnapshot {
                decision,
                updated_at: now,
            });
        let Some(interval) = interval else {
            return Ok(());
        };
        thread::sleep(interval);
    }
}

fn monitor_router(state: MonitorHttpState) -> Router {
    Router::new()
        .route("/api/v3.6/health", get(monitor_health))
        .route(
            "/api/v3.6/funding-coins/{funding_coin_id}/monitor-status",
            get(monitor_status),
        )
        .with_state(state)
}

async fn monitor_health(
    State(state): State<MonitorHttpState>,
) -> (StatusCode, Json<MonitorStatusResponse>) {
    status_response(&state)
}

async fn monitor_status(
    State(state): State<MonitorHttpState>,
    Path(funding_coin_id): Path<String>,
    Query(query): Query<MonitorStatusQuery>,
) -> (StatusCode, Json<MonitorStatusResponse>) {
    if query.protocol_version != PROTOCOL_VERSION_TEXT
        || funding_coin_id.trim_start_matches("0x").to_lowercase() != state.funding_coin_id
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_status(
                &state,
                "protocol version or Funding Coin ID mismatch",
            )),
        );
    }
    status_response(&state)
}

fn status_response(state: &MonitorHttpState) -> (StatusCode, Json<MonitorStatusResponse>) {
    let now = unix_now().unwrap_or(u64::MAX);
    let snapshot = state.snapshot.read().ok().and_then(|value| value.clone());
    let Some(snapshot) = snapshot else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_status(state, "initial chain poll has not completed")),
        );
    };
    let age = now.saturating_sub(snapshot.updated_at);
    let fresh = age <= state.max_staleness_seconds;
    let rpc_known = snapshot.decision.action != MonitorAction::Unknown;
    let (status, status_code) = if !fresh {
        ("STALE", StatusCode::SERVICE_UNAVAILABLE)
    } else if !rpc_known {
        ("UNKNOWN", StatusCode::SERVICE_UNAVAILABLE)
    } else {
        ("READY", StatusCode::OK)
    };
    let (alert_level, operator_attention_required) = alert_classification(snapshot.decision.action);
    (
        status_code,
        Json(MonitorStatusResponse {
            protocol_version: PROTOCOL_VERSION_TEXT,
            service: "watchtower-readonly-monitor",
            status,
            funding_coin_id: state.funding_coin_id.clone(),
            rpc_mode: "READ_ONLY",
            interval_seconds: state.interval_seconds,
            max_staleness_seconds: state.max_staleness_seconds,
            last_poll_at: Some(snapshot.updated_at),
            age_seconds: Some(age),
            fresh,
            action: Some(snapshot.decision.action),
            alert_level,
            operator_attention_required,
            detail: snapshot.decision.detail,
            peak_height: snapshot.decision.peak_height,
            current_state_sequence: snapshot.decision.current_state_sequence,
            latest_state_sequence: snapshot.decision.latest_state_sequence,
            challenge_deadline_height: snapshot.decision.challenge_deadline_height,
            manual_approval_required: true,
            execution_gate: EXECUTION_GATE,
            spend_bundle_created: snapshot.decision.spend_bundle_created,
            broadcast_enabled: false,
            broadcast_ready: snapshot.decision.broadcast_ready,
            chain_broadcast: snapshot.decision.chain_broadcast,
        }),
    )
}

fn error_status(state: &MonitorHttpState, detail: &str) -> MonitorStatusResponse {
    MonitorStatusResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        service: "watchtower-readonly-monitor",
        status: "STARTING",
        funding_coin_id: state.funding_coin_id.clone(),
        rpc_mode: "READ_ONLY",
        interval_seconds: state.interval_seconds,
        max_staleness_seconds: state.max_staleness_seconds,
        last_poll_at: None,
        age_seconds: None,
        fresh: false,
        action: None,
        alert_level: "CRITICAL",
        operator_attention_required: true,
        detail: detail.into(),
        peak_height: None,
        current_state_sequence: None,
        latest_state_sequence: None,
        challenge_deadline_height: None,
        manual_approval_required: true,
        execution_gate: EXECUTION_GATE,
        spend_bundle_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    }
}

fn alert_classification(action: MonitorAction) -> (&'static str, bool) {
    match action {
        MonitorAction::FundingOpen | MonitorAction::Finalized => ("NONE", false),
        MonitorAction::ClosingCurrent => ("WARNING", true),
        MonitorAction::ChallengePlanned
        | MonitorAction::ChallengeAlreadyPlanned
        | MonitorAction::DeadlinePassed
        | MonitorAction::ReorgPending
        | MonitorAction::Unknown => ("CRITICAL", true),
    }
}

fn parse_interval(value: Option<&String>) -> Result<Option<Duration>, String> {
    value
        .map(|value| {
            let seconds = value
                .parse::<u64>()
                .map_err(|_| "INTERVAL_SECONDS must be a positive u64")?;
            if seconds == 0 {
                return Err("INTERVAL_SECONDS must be a positive u64".into());
            }
            Ok(Duration::from_secs(seconds))
        })
        .transpose()
}

fn parse_listen(value: Option<&String>) -> Result<Option<SocketAddr>, String> {
    value
        .map(|value| {
            let listen = value
                .parse::<SocketAddr>()
                .map_err(|_| "LISTEN must be an IP socket address")?;
            if !listen.ip().is_loopback() {
                return Err("LISTEN must use a loopback address".into());
            }
            Ok(listen)
        })
        .transpose()
}

fn parse_bytes32(value: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| error.to_string())?;
    bytes
        .try_into()
        .map_err(|_| "FUNDING_COIN_ID must encode exactly 32 bytes".into())
}

fn unix_now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    fn decision(action: MonitorAction) -> MonitorDecision {
        MonitorDecision {
            action,
            funding_coin_id: "42".repeat(32),
            peak_height: Some(100),
            current_state_sequence: None,
            latest_state_sequence: Some(1),
            challenge_deadline_height: None,
            detail: "test decision".into(),
            spend_bundle_created: false,
            broadcast_ready: false,
            chain_broadcast: false,
            challenge: None,
        }
    }

    fn state(action: Option<MonitorAction>, updated_at: u64) -> MonitorHttpState {
        MonitorHttpState {
            funding_coin_id: "42".repeat(32),
            interval_seconds: 300,
            max_staleness_seconds: 900,
            snapshot: Arc::new(RwLock::new(action.map(|action| MonitorSnapshot {
                decision: decision(action),
                updated_at,
            }))),
        }
    }

    #[test]
    fn interval_is_optional_and_must_be_positive() {
        assert_eq!(parse_interval(None).expect("one shot"), None);
        assert_eq!(
            parse_interval(Some(&"300".into())).expect("interval"),
            Some(Duration::from_secs(300))
        );
        assert!(parse_interval(Some(&"0".into())).is_err());
        assert!(parse_interval(Some(&"invalid".into())).is_err());
    }

    #[test]
    fn status_listener_is_loopback_only() {
        assert_eq!(
            parse_listen(Some(&"127.0.0.1:18741".into())).expect("listen"),
            Some("127.0.0.1:18741".parse().expect("socket"))
        );
        assert!(parse_listen(Some(&"0.0.0.0:18741".into())).is_err());
        assert!(parse_listen(Some(&"43.156.47.252:18741".into())).is_err());
    }

    #[test]
    fn alert_levels_require_attention_for_chain_transitions() {
        assert_eq!(
            alert_classification(MonitorAction::FundingOpen),
            ("NONE", false)
        );
        assert_eq!(
            alert_classification(MonitorAction::ClosingCurrent),
            ("WARNING", true)
        );
        assert_eq!(
            alert_classification(MonitorAction::ChallengePlanned),
            ("CRITICAL", true)
        );
        assert_eq!(
            alert_classification(MonitorAction::Unknown),
            ("CRITICAL", true)
        );
    }

    #[tokio::test]
    async fn versioned_status_is_read_only_and_fail_closed() {
        let now = unix_now().expect("now");
        let app = monitor_router(state(Some(MonitorAction::FundingOpen), now));
        let coin = "42".repeat(32);
        let response = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/v3.6/funding-coins/{coin}/monitor-status?protocol_version=0x0360"
                ))
                .body(Body::empty())
                .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["status"], "READY");
        assert_eq!(body["action"], "FUNDING_OPEN");
        assert_eq!(body["execution_gate"], EXECUTION_GATE);
        assert_eq!(body["spend_bundle_created"], false);
        assert_eq!(body["broadcast_enabled"], false);
        assert_eq!(body["broadcast_ready"], false);
        assert_eq!(body["chain_broadcast"], false);

        let wrong_version = app
            .oneshot(
                Request::get(format!(
                    "/api/v3.6/funding-coins/{coin}/monitor-status?protocol_version=0x9999"
                ))
                .body(Body::empty())
                .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_version.status(), StatusCode::BAD_REQUEST);

        let stale = monitor_router(state(Some(MonitorAction::FundingOpen), 1))
            .oneshot(
                Request::get("/api/v3.6/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(stale.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
