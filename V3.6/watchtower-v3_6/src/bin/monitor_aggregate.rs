use std::{
    collections::HashSet,
    fs,
    net::{IpAddr, SocketAddr},
    path::Path as FilePath,
    sync::{Arc, Mutex, RwLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::{Url, blocking::Client};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use xhub_protocol_v3_6::{Bytes32, sha256_parts};
use xhub_watchtower_v3_6::monitor::MonitorAction;
use zeroize::Zeroize;

const PROTOCOL_VERSION_TEXT: &str = "0x0360";
const EXECUTION_GATE: &str = "DUAL_APPROVAL_AND_FINAL_RECHECK_REQUIRED";
const QUORUM_REQUIRED: usize = 2;

#[derive(Clone)]
struct AggregateHttpState {
    funding_coin_id: String,
    interval_seconds: u64,
    max_staleness_seconds: u64,
    snapshot: Arc<RwLock<Option<AggregateDecision>>>,
    alerts: Arc<Mutex<AlertStore>>,
    acknowledgement_token_hash: Bytes32,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteMonitorStatus {
    protocol_version: String,
    status: String,
    funding_coin_id: String,
    rpc_mode: String,
    fresh: bool,
    action: Option<MonitorAction>,
    alert_level: String,
    operator_attention_required: bool,
    peak_height: Option<u64>,
    last_poll_at: Option<u64>,
    manual_approval_required: bool,
    execution_gate: String,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Clone, Serialize)]
struct EndpointResult {
    endpoint: String,
    reachable: bool,
    http_status: Option<u16>,
    eligible: bool,
    status: Option<String>,
    action: Option<MonitorAction>,
    alert_level: Option<String>,
    operator_attention_required: Option<bool>,
    peak_height: Option<u64>,
    last_poll_at: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AggregateDecision {
    protocol_version: &'static str,
    service: &'static str,
    status: &'static str,
    funding_coin_id: String,
    observed_at: u64,
    interval_seconds: u64,
    max_staleness_seconds: u64,
    endpoint_count: usize,
    eligible_count: usize,
    agreeing_count: usize,
    quorum_required: usize,
    quorum_reached: bool,
    consensus_action: Option<MonitorAction>,
    consensus_peak_height: Option<u64>,
    alert_level: &'static str,
    operator_attention_required: bool,
    detail: String,
    endpoints: Vec<EndpointResult>,
    manual_approval_required: bool,
    execution_gate: &'static str,
    physical_failure_domain_count: u8,
    production_ready: bool,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct AggregateStatusResponse {
    #[serde(flatten)]
    decision: AggregateDecision,
    age_seconds: u64,
    fresh: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    protocol_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlertQuery {
    protocol_version: String,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcknowledgeRequest {
    protocol_version: String,
    operator_id: String,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AlertEvent {
    event_id: u64,
    fingerprint: String,
    status: String,
    consensus_action: Option<String>,
    alert_level: String,
    operator_attention_required: bool,
    quorum_reached: bool,
    eligible_count: u64,
    agreeing_count: u64,
    first_seen: u64,
    last_seen: u64,
    occurrence_count: u64,
    acknowledged_at: Option<u64>,
    acknowledged_by: Option<String>,
    acknowledgement_note: Option<String>,
    resolved_at: Option<u64>,
    snapshot_json: String,
}

#[derive(Debug, Serialize)]
struct AlertListResponse {
    protocol_version: &'static str,
    events: Vec<AlertEvent>,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct AlertMutationResponse {
    protocol_version: &'static str,
    status: &'static str,
    event: AlertEvent,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    protocol_version: &'static str,
    code: &'static str,
    message: String,
    accepted: bool,
    spend_bundle_created: bool,
    broadcast_enabled: bool,
    broadcast_ready: bool,
    chain_broadcast: bool,
}

struct AlertStore {
    connection: Connection,
}

impl AlertStore {
    fn open(path: impl AsRef<FilePath>) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, String> {
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE IF NOT EXISTS v36_monitor_alert_events (
                   event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   fingerprint TEXT NOT NULL,
                   status TEXT NOT NULL,
                   consensus_action TEXT,
                   alert_level TEXT NOT NULL,
                   operator_attention_required INTEGER NOT NULL CHECK(operator_attention_required IN (0, 1)),
                   quorum_reached INTEGER NOT NULL CHECK(quorum_reached IN (0, 1)),
                   eligible_count INTEGER NOT NULL CHECK(eligible_count >= 0),
                   agreeing_count INTEGER NOT NULL CHECK(agreeing_count >= 0),
                   first_seen INTEGER NOT NULL CHECK(first_seen >= 0),
                   last_seen INTEGER NOT NULL CHECK(last_seen >= first_seen),
                   occurrence_count INTEGER NOT NULL CHECK(occurrence_count > 0),
                   acknowledged_at INTEGER,
                   acknowledged_by TEXT,
                   acknowledgement_note TEXT,
                   resolved_at INTEGER,
                   snapshot_json TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS v36_monitor_alert_events_last_seen
                   ON v36_monitor_alert_events(last_seen DESC);",
            )
            .map_err(|error| error.to_string())?;
        Ok(Self { connection })
    }

    fn record(&mut self, decision: &AggregateDecision) -> Result<(AlertEvent, bool), String> {
        let fingerprint = alert_fingerprint(decision);
        let snapshot_json = serde_json::to_string(decision).map_err(|error| error.to_string())?;
        if let Some(latest) = self.latest()? {
            if latest.fingerprint == fingerprint {
                self.connection
                    .execute(
                        "UPDATE v36_monitor_alert_events
                         SET last_seen=?2, occurrence_count=occurrence_count + 1, snapshot_json=?3
                         WHERE event_id=?1",
                        params![
                            to_i64(latest.event_id)?,
                            to_i64(decision.observed_at)?,
                            snapshot_json,
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                return Ok((self.event(latest.event_id)?, false));
            }
            if latest.operator_attention_required && latest.resolved_at.is_none() {
                self.connection
                    .execute(
                        "UPDATE v36_monitor_alert_events SET resolved_at=?2 WHERE event_id=?1",
                        params![to_i64(latest.event_id)?, to_i64(decision.observed_at)?],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        let action = decision.consensus_action.map(action_text);
        self.connection
            .execute(
                "INSERT INTO v36_monitor_alert_events (
                   fingerprint, status, consensus_action, alert_level,
                   operator_attention_required, quorum_reached, eligible_count,
                   agreeing_count, first_seen, last_seen, occurrence_count, snapshot_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 1, ?10)",
                params![
                    fingerprint,
                    decision.status,
                    action,
                    decision.alert_level,
                    decision.operator_attention_required,
                    decision.quorum_reached,
                    to_i64(
                        u64::try_from(decision.eligible_count)
                            .map_err(|_| "eligible_count exceeds u64")?
                    )?,
                    to_i64(
                        u64::try_from(decision.agreeing_count)
                            .map_err(|_| "agreeing_count exceeds u64")?
                    )?,
                    to_i64(decision.observed_at)?,
                    snapshot_json,
                ],
            )
            .map_err(|error| error.to_string())?;
        let event_id = u64::try_from(self.connection.last_insert_rowid())
            .map_err(|_| "SQLite returned a negative alert event ID")?;
        Ok((self.event(event_id)?, true))
    }

    fn acknowledge(
        &mut self,
        event_id: u64,
        operator_id: &str,
        note: &str,
        now: u64,
    ) -> Result<AlertEvent, String> {
        validate_operator_text("operator_id", operator_id, 64)?;
        validate_operator_text("note", note, 256)?;
        let event = self.event(event_id)?;
        if !event.operator_attention_required {
            return Err("event does not require operator acknowledgement".into());
        }
        if event.resolved_at.is_some() {
            return Err("resolved event cannot be acknowledged".into());
        }
        if event.acknowledged_at.is_some() {
            if event.acknowledged_by.as_deref() == Some(operator_id)
                && event.acknowledgement_note.as_deref() == Some(note)
            {
                return Ok(event);
            }
            return Err("event was already acknowledged with different operator data".into());
        }
        self.connection
            .execute(
                "UPDATE v36_monitor_alert_events
                 SET acknowledged_at=?2, acknowledged_by=?3, acknowledgement_note=?4
                 WHERE event_id=?1",
                params![to_i64(event_id)?, to_i64(now)?, operator_id, note],
            )
            .map_err(|error| error.to_string())?;
        self.event(event_id)
    }

    fn latest(&self) -> Result<Option<AlertEvent>, String> {
        self.connection
            .query_row(
                "SELECT event_id, fingerprint, status, consensus_action, alert_level,
                    operator_attention_required, quorum_reached, eligible_count,
                    agreeing_count, first_seen, last_seen, occurrence_count,
                    acknowledged_at, acknowledged_by, acknowledgement_note,
                    resolved_at, snapshot_json
                 FROM v36_monitor_alert_events ORDER BY event_id DESC LIMIT 1",
                [],
                alert_event_from_row,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn event(&self, event_id: u64) -> Result<AlertEvent, String> {
        self.connection
            .query_row(
                "SELECT event_id, fingerprint, status, consensus_action, alert_level,
                    operator_attention_required, quorum_reached, eligible_count,
                    agreeing_count, first_seen, last_seen, occurrence_count,
                    acknowledged_at, acknowledged_by, acknowledgement_note,
                    resolved_at, snapshot_json
                 FROM v36_monitor_alert_events WHERE event_id=?1",
                [to_i64(event_id)?],
                alert_event_from_row,
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "alert event was not found".into())
    }

    fn events(&self, limit: u16) -> Result<Vec<AlertEvent>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_id, fingerprint, status, consensus_action, alert_level,
                    operator_attention_required, quorum_reached, eligible_count,
                    agreeing_count, first_seen, last_seen, occurrence_count,
                    acknowledged_at, acknowledged_by, acknowledgement_note,
                    resolved_at, snapshot_json
                 FROM v36_monitor_alert_events ORDER BY event_id DESC LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map([i64::from(limit)], alert_event_from_row)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }
}

fn alert_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AlertEvent> {
    Ok(AlertEvent {
        event_id: row_u64(row, 0)?,
        fingerprint: row.get(1)?,
        status: row.get(2)?,
        consensus_action: row.get(3)?,
        alert_level: row.get(4)?,
        operator_attention_required: row.get(5)?,
        quorum_reached: row.get(6)?,
        eligible_count: row_u64(row, 7)?,
        agreeing_count: row_u64(row, 8)?,
        first_seen: row_u64(row, 9)?,
        last_seen: row_u64(row, 10)?,
        occurrence_count: row_u64(row, 11)?,
        acknowledged_at: row_optional_u64(row, 12)?,
        acknowledged_by: row.get(13)?,
        acknowledgement_note: row.get(14)?,
        resolved_at: row_optional_u64(row, 15)?,
        snapshot_json: row.get(16)?,
    })
}

fn row_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn row_optional_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
        })
        .transpose()
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Watchtower monitor aggregator failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 8 {
        return Err(
            "usage: watchtower-monitor-aggregate-v3-6 ALERT_DB ACK_TOKEN_FILE FUNDING_COIN_ID INTERVAL_SECONDS LISTEN MONITOR_A_BASE_URL MONITOR_B_BASE_URL MONITOR_C_BASE_URL"
                .into(),
        );
    }
    let alerts = Arc::new(Mutex::new(AlertStore::open(&args[0])?));
    let acknowledgement_token_hash = read_token_hash(&args[1])?;
    let funding_coin_id = parse_bytes32_text(&args[2])?;
    let interval = parse_positive_duration(&args[3])?;
    let listen = parse_listen(&args[4])?;
    let endpoints = args[5..]
        .iter()
        .map(|value| parse_endpoint(value))
        .collect::<Result<Vec<_>, _>>()?;
    if endpoints
        .iter()
        .map(Url::port)
        .collect::<HashSet<_>>()
        .len()
        != endpoints.len()
    {
        return Err("monitor endpoints must use distinct loopback ports".into());
    }

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| error.to_string())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let snapshot = Arc::new(RwLock::new(None));
    let http_state = AggregateHttpState {
        funding_coin_id: funding_coin_id.clone(),
        interval_seconds: interval.as_secs(),
        max_staleness_seconds: interval.as_secs().saturating_mul(3),
        snapshot: Arc::clone(&snapshot),
        alerts: Arc::clone(&alerts),
        acknowledgement_token_hash,
    };
    thread::spawn(move || {
        if let Err(error) = aggregate_loop(
            client,
            funding_coin_id,
            interval,
            endpoints,
            snapshot,
            alerts,
        ) {
            eprintln!("Watchtower monitor aggregation failed: {error}");
            std::process::exit(1);
        }
    });
    println!("XHUB Watchtower V3.6 monitor aggregator listening on http://{listen}");
    axum::serve(listener, aggregate_router(http_state))
        .await
        .map_err(|error| error.to_string())
}

fn aggregate_loop(
    client: Client,
    funding_coin_id: String,
    interval: Duration,
    endpoints: Vec<Url>,
    snapshot: Arc<RwLock<Option<AggregateDecision>>>,
    alerts: Arc<Mutex<AlertStore>>,
) -> Result<(), String> {
    loop {
        let now = unix_now()?;
        let results = endpoints
            .iter()
            .map(|endpoint| fetch_endpoint(&client, endpoint, &funding_coin_id))
            .collect();
        let decision = aggregate_results(&funding_coin_id, interval.as_secs(), now, results);
        let (event, created) = alerts
            .lock()
            .map_err(|error| format!("alert store lock poisoned: {error}"))?
            .record(&decision)?;
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "aggregate": &decision,
                "alert_event_id": event.event_id,
                "alert_event_created": created,
                "alert_occurrence_count": event.occurrence_count,
                "alert_acknowledged": event.acknowledged_at.is_some(),
                "alert_resolved": event.resolved_at.is_some()
            }))
            .map_err(|error| error.to_string())?
        );
        *snapshot
            .write()
            .map_err(|error| format!("aggregate snapshot lock poisoned: {error}"))? =
            Some(decision);
        thread::sleep(interval);
    }
}

fn fetch_endpoint(client: &Client, endpoint: &Url, funding_coin_id: &str) -> EndpointResult {
    let endpoint_text = endpoint.as_str().trim_end_matches('/').to_string();
    let url = format!(
        "{endpoint_text}/api/v3.6/funding-coins/{funding_coin_id}/monitor-status?protocol_version={PROTOCOL_VERSION_TEXT}"
    );
    let response = match client.get(url).send() {
        Ok(response) => response,
        Err(error) => return endpoint_error(endpoint_text, None, error.to_string()),
    };
    let http_status = response.status().as_u16();
    let body = match response.json::<RemoteMonitorStatus>() {
        Ok(body) => body,
        Err(error) => return endpoint_error(endpoint_text, Some(http_status), error.to_string()),
    };
    let validation_error = validate_remote_status(&body, funding_coin_id).err();
    EndpointResult {
        endpoint: endpoint_text,
        reachable: true,
        http_status: Some(http_status),
        eligible: (200..300).contains(&http_status) && validation_error.is_none(),
        status: Some(body.status),
        action: body.action,
        alert_level: Some(body.alert_level),
        operator_attention_required: Some(body.operator_attention_required),
        peak_height: body.peak_height,
        last_poll_at: body.last_poll_at,
        error: validation_error,
    }
}

fn validate_remote_status(
    status: &RemoteMonitorStatus,
    funding_coin_id: &str,
) -> Result<(), String> {
    if status.protocol_version != PROTOCOL_VERSION_TEXT
        || status
            .funding_coin_id
            .trim_start_matches("0x")
            .to_lowercase()
            != funding_coin_id
    {
        return Err("protocol version or Funding Coin ID mismatch".into());
    }
    if status.status != "READY" || !status.fresh || status.rpc_mode != "READ_ONLY" {
        return Err("monitor is not READY with a fresh read-only chain view".into());
    }
    if status.action.is_none() {
        return Err("monitor omitted its chain action".into());
    }
    if !status.manual_approval_required || status.execution_gate != EXECUTION_GATE {
        return Err("monitor weakened the manual execution gate".into());
    }
    if status.spend_bundle_created
        || status.broadcast_enabled
        || status.broadcast_ready
        || status.chain_broadcast
    {
        return Err("monitor reported a prohibited execution or broadcast capability".into());
    }
    Ok(())
}

fn endpoint_error(endpoint: String, http_status: Option<u16>, error: String) -> EndpointResult {
    EndpointResult {
        endpoint,
        reachable: http_status.is_some(),
        http_status,
        eligible: false,
        status: None,
        action: None,
        alert_level: None,
        operator_attention_required: None,
        peak_height: None,
        last_poll_at: None,
        error: Some(error),
    }
}

fn aggregate_results(
    funding_coin_id: &str,
    interval_seconds: u64,
    now: u64,
    endpoints: Vec<EndpointResult>,
) -> AggregateDecision {
    let eligible = endpoints
        .iter()
        .filter(|endpoint| endpoint.eligible)
        .collect::<Vec<_>>();
    let eligible_count = eligible.len();
    let mut consensus_action = None;
    let mut agreeing_count = 0;
    for candidate in eligible.iter().filter_map(|endpoint| endpoint.action) {
        let count = eligible
            .iter()
            .filter(|endpoint| endpoint.action == Some(candidate))
            .count();
        if count > agreeing_count {
            consensus_action = Some(candidate);
            agreeing_count = count;
        }
    }
    let quorum_reached = agreeing_count >= QUORUM_REQUIRED;
    let exact_agreement = quorum_reached && agreeing_count == endpoints.len();
    let status = if exact_agreement {
        "READY"
    } else if quorum_reached {
        "DEGRADED"
    } else {
        "UNKNOWN"
    };
    let consensus_peak_height = consensus_action.and_then(|action| {
        eligible
            .iter()
            .filter(|endpoint| endpoint.action == Some(action))
            .filter_map(|endpoint| endpoint.peak_height)
            .max()
    });
    let (base_alert, base_attention) = consensus_action
        .map(alert_classification)
        .unwrap_or(("CRITICAL", true));
    let alert_level = if !quorum_reached || base_alert == "CRITICAL" {
        "CRITICAL"
    } else if !exact_agreement || base_alert == "WARNING" {
        "WARNING"
    } else {
        "NONE"
    };
    let operator_attention_required = base_attention || !exact_agreement;
    let detail = if exact_agreement {
        format!(
            "{}-of-{} monitor status agreement",
            agreeing_count,
            endpoints.len()
        )
    } else if quorum_reached {
        format!(
            "{}-of-{} quorum reached with endpoint failure or disagreement",
            agreeing_count,
            endpoints.len()
        )
    } else {
        format!(
            "no {}-of-{} monitor status quorum",
            QUORUM_REQUIRED,
            endpoints.len()
        )
    };
    AggregateDecision {
        protocol_version: PROTOCOL_VERSION_TEXT,
        service: "watchtower-monitor-aggregator",
        status,
        funding_coin_id: funding_coin_id.into(),
        observed_at: now,
        interval_seconds,
        max_staleness_seconds: interval_seconds.saturating_mul(3),
        endpoint_count: endpoints.len(),
        eligible_count,
        agreeing_count,
        quorum_required: QUORUM_REQUIRED,
        quorum_reached,
        consensus_action,
        consensus_peak_height,
        alert_level,
        operator_attention_required,
        detail,
        endpoints,
        manual_approval_required: true,
        execution_gate: EXECUTION_GATE,
        physical_failure_domain_count: 1,
        production_ready: false,
        spend_bundle_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    }
}

fn aggregate_router(state: AggregateHttpState) -> Router {
    Router::new()
        .route("/api/v3.6/health", get(aggregate_health))
        .route("/api/v3.6/monitor-aggregate", get(aggregate_status))
        .route("/api/v3.6/alerts", get(list_alerts))
        .route("/api/v3.6/alerts/{event_id}", get(get_alert))
        .route(
            "/api/v3.6/alerts/{event_id}/acknowledge",
            post(acknowledge_alert),
        )
        .with_state(state)
}

async fn list_alerts(
    State(state): State<AggregateHttpState>,
    Query(query): Query<AlertQuery>,
) -> Response {
    if query.protocol_version != PROTOCOL_VERSION_TEXT {
        return api_error(
            StatusCode::BAD_REQUEST,
            "PROTOCOL_VERSION_MISMATCH",
            "protocol version mismatch",
        );
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let events = match state.alerts.lock() {
        Ok(store) => match store.events(limit) {
            Ok(events) => events,
            Err(error) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ALERT_STORE_ERROR",
                    error,
                );
            }
        },
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ALERT_STORE_ERROR",
                format!("alert store lock poisoned: {error}"),
            );
        }
    };
    Json(AlertListResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        events,
        spend_bundle_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    })
    .into_response()
}

async fn get_alert(
    State(state): State<AggregateHttpState>,
    Path(event_id): Path<u64>,
    Query(query): Query<VersionQuery>,
) -> Response {
    if query.protocol_version != PROTOCOL_VERSION_TEXT {
        return api_error(
            StatusCode::BAD_REQUEST,
            "PROTOCOL_VERSION_MISMATCH",
            "protocol version mismatch",
        );
    }
    let event = match state.alerts.lock() {
        Ok(store) => match store.event(event_id) {
            Ok(event) => event,
            Err(error) if error == "alert event was not found" => {
                return api_error(StatusCode::NOT_FOUND, "ALERT_NOT_FOUND", error);
            }
            Err(error) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ALERT_STORE_ERROR",
                    error,
                );
            }
        },
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ALERT_STORE_ERROR",
                format!("alert store lock poisoned: {error}"),
            );
        }
    };
    alert_response("FOUND", event)
}

async fn acknowledge_alert(
    State(state): State<AggregateHttpState>,
    Path(event_id): Path<u64>,
    headers: HeaderMap,
    Json(request): Json<AcknowledgeRequest>,
) -> Response {
    if !authorized(&headers, &state.acknowledgement_token_hash) {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "a valid Bearer token is required",
        );
    }
    if request.protocol_version != PROTOCOL_VERSION_TEXT {
        return api_error(
            StatusCode::BAD_REQUEST,
            "PROTOCOL_VERSION_MISMATCH",
            "protocol version mismatch",
        );
    }
    let now = match unix_now() {
        Ok(now) => now,
        Err(error) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "CLOCK_ERROR", error);
        }
    };
    let event = match state.alerts.lock() {
        Ok(mut store) => {
            match store.acknowledge(event_id, &request.operator_id, &request.note, now) {
                Ok(event) => event,
                Err(error) if error == "alert event was not found" => {
                    return api_error(StatusCode::NOT_FOUND, "ALERT_NOT_FOUND", error);
                }
                Err(error) => {
                    return api_error(StatusCode::CONFLICT, "ACKNOWLEDGEMENT_REJECTED", error);
                }
            }
        }
        Err(error) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ALERT_STORE_ERROR",
                format!("alert store lock poisoned: {error}"),
            );
        }
    };
    alert_response("ACKNOWLEDGED", event)
}

fn alert_response(status: &'static str, event: AlertEvent) -> Response {
    Json(AlertMutationResponse {
        protocol_version: PROTOCOL_VERSION_TEXT,
        status,
        event,
        spend_bundle_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    })
    .into_response()
}

fn api_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErrorBody {
            protocol_version: PROTOCOL_VERSION_TEXT,
            code,
            message: message.into(),
            accepted: false,
            spend_bundle_created: false,
            broadcast_enabled: false,
            broadcast_ready: false,
            chain_broadcast: false,
        }),
    )
        .into_response()
}

async fn aggregate_health(
    State(state): State<AggregateHttpState>,
) -> (StatusCode, Json<AggregateStatusResponse>) {
    aggregate_response(&state)
}

async fn aggregate_status(
    State(state): State<AggregateHttpState>,
    Query(query): Query<VersionQuery>,
) -> (StatusCode, Json<AggregateStatusResponse>) {
    if query.protocol_version != PROTOCOL_VERSION_TEXT {
        let (_, body) = unavailable_response(&state, "protocol version mismatch");
        return (StatusCode::BAD_REQUEST, body);
    }
    aggregate_response(&state)
}

fn aggregate_response(state: &AggregateHttpState) -> (StatusCode, Json<AggregateStatusResponse>) {
    let now = unix_now().unwrap_or(u64::MAX);
    let decision = state.snapshot.read().ok().and_then(|value| value.clone());
    let Some(mut decision) = decision else {
        return unavailable_response(state, "initial aggregate poll has not completed");
    };
    let age_seconds = now.saturating_sub(decision.observed_at);
    let fresh = age_seconds <= state.max_staleness_seconds;
    if !fresh {
        decision.status = "STALE";
        decision.alert_level = "CRITICAL";
        decision.operator_attention_required = true;
        decision.detail = "aggregate monitor state exceeded the staleness limit".into();
    }
    let healthy = fresh && decision.quorum_reached;
    (
        if healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(AggregateStatusResponse {
            decision,
            age_seconds,
            fresh,
        }),
    )
}

fn unavailable_response(
    state: &AggregateHttpState,
    detail: &str,
) -> (StatusCode, Json<AggregateStatusResponse>) {
    let now = unix_now().unwrap_or(u64::MAX);
    let decision = AggregateDecision {
        protocol_version: PROTOCOL_VERSION_TEXT,
        service: "watchtower-monitor-aggregator",
        status: "UNKNOWN",
        funding_coin_id: state.funding_coin_id.clone(),
        observed_at: now,
        interval_seconds: state.interval_seconds,
        max_staleness_seconds: state.max_staleness_seconds,
        endpoint_count: 3,
        eligible_count: 0,
        agreeing_count: 0,
        quorum_required: QUORUM_REQUIRED,
        quorum_reached: false,
        consensus_action: None,
        consensus_peak_height: None,
        alert_level: "CRITICAL",
        operator_attention_required: true,
        detail: detail.into(),
        endpoints: Vec::new(),
        manual_approval_required: true,
        execution_gate: EXECUTION_GATE,
        physical_failure_domain_count: 1,
        production_ready: false,
        spend_bundle_created: false,
        broadcast_enabled: false,
        broadcast_ready: false,
        chain_broadcast: false,
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(AggregateStatusResponse {
            decision,
            age_seconds: 0,
            fresh: false,
        }),
    )
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

fn read_token_hash(path: impl AsRef<FilePath>) -> Result<Bytes32, String> {
    let mut token = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let trimmed = token.trim();
    if trimmed.is_empty() {
        token.zeroize();
        return Err("ACK_TOKEN_FILE must contain a non-empty token".into());
    }
    let token_hash = sha256_parts(&[
        b"XHUB_MONITOR_AGGREGATE_ACK_BEARER_V3_6",
        trimmed.as_bytes(),
    ]);
    token.zeroize();
    Ok(token_hash)
}

fn authorized(headers: &HeaderMap, expected_hash: &Bytes32) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .map(|token| sha256_parts(&[b"XHUB_MONITOR_AGGREGATE_ACK_BEARER_V3_6", token.as_bytes()]))
        .is_some_and(|actual_hash| constant_time_eq(&actual_hash, expected_hash))
}

fn constant_time_eq(left: &Bytes32, right: &Bytes32) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("value {value} exceeds SQLite INTEGER range"))
}

fn alert_fingerprint(decision: &AggregateDecision) -> String {
    let endpoint_states = decision
        .endpoints
        .iter()
        .map(|endpoint| {
            serde_json::json!({
                "endpoint": endpoint.endpoint,
                "reachable": endpoint.reachable,
                "eligible": endpoint.eligible,
                "status": endpoint.status,
                "action": endpoint.action.map(action_text),
                "alert_level": endpoint.alert_level,
                "operator_attention_required": endpoint.operator_attention_required,
                "error": endpoint.error,
            })
        })
        .collect::<Vec<_>>();
    let material = serde_json::to_vec(&serde_json::json!({
        "protocol_version": decision.protocol_version,
        "funding_coin_id": decision.funding_coin_id,
        "status": decision.status,
        "eligible_count": decision.eligible_count,
        "agreeing_count": decision.agreeing_count,
        "quorum_reached": decision.quorum_reached,
        "consensus_action": decision.consensus_action.map(action_text),
        "alert_level": decision.alert_level,
        "operator_attention_required": decision.operator_attention_required,
        "endpoints": endpoint_states,
    }))
    .expect("alert fingerprint material is JSON serializable");
    hex::encode(sha256_parts(&[
        b"XHUB_MONITOR_ALERT_FINGERPRINT_V3_6",
        &material,
    ]))
}

const fn action_text(action: MonitorAction) -> &'static str {
    match action {
        MonitorAction::FundingOpen => "FUNDING_OPEN",
        MonitorAction::ClosingCurrent => "CLOSING_CURRENT",
        MonitorAction::ChallengePlanned => "CHALLENGE_PLANNED",
        MonitorAction::ChallengeAlreadyPlanned => "CHALLENGE_ALREADY_PLANNED",
        MonitorAction::DeadlinePassed => "DEADLINE_PASSED",
        MonitorAction::Finalized => "FINALIZED",
        MonitorAction::ReorgPending => "REORG_PENDING",
        MonitorAction::Unknown => "UNKNOWN",
    }
}

fn validate_operator_text(field: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.trim() != value {
        return Err(format!(
            "{field} must be non-empty and have no surrounding whitespace"
        ));
    }
    if value.len() > max_bytes {
        return Err(format!("{field} must not exceed {max_bytes} UTF-8 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn parse_bytes32_text(value: &str) -> Result<String, String> {
    let value = value.strip_prefix("0x").unwrap_or(value).to_lowercase();
    let bytes = hex::decode(&value).map_err(|error| error.to_string())?;
    if bytes.len() != 32 {
        return Err("FUNDING_COIN_ID must encode exactly 32 bytes".into());
    }
    Ok(value)
}

fn parse_positive_duration(value: &str) -> Result<Duration, String> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "INTERVAL_SECONDS must be a positive u64")?;
    if seconds == 0 {
        return Err("INTERVAL_SECONDS must be a positive u64".into());
    }
    Ok(Duration::from_secs(seconds))
}

fn parse_listen(value: &str) -> Result<SocketAddr, String> {
    let listen = value
        .parse::<SocketAddr>()
        .map_err(|_| "LISTEN must be an IP socket address")?;
    if !listen.ip().is_loopback() {
        return Err("LISTEN must use a loopback address".into());
    }
    Ok(listen)
}

fn parse_endpoint(value: &str) -> Result<Url, String> {
    let endpoint = Url::parse(value).map_err(|error| error.to_string())?;
    let host = endpoint
        .host_str()
        .ok_or("monitor endpoint must include an IP host")?
        .parse::<IpAddr>()
        .map_err(|_| "monitor endpoint host must be an IP address")?;
    if endpoint.scheme() != "http"
        || !host.is_loopback()
        || endpoint.port().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.path() != "/"
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err("monitor endpoint must be a credential-free loopback HTTP origin".into());
    }
    Ok(endpoint)
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

    fn endpoint(name: &str, action: MonitorAction) -> EndpointResult {
        EndpointResult {
            endpoint: name.into(),
            reachable: true,
            http_status: Some(200),
            eligible: true,
            status: Some("READY".into()),
            action: Some(action),
            alert_level: Some("NONE".into()),
            operator_attention_required: Some(false),
            peak_height: Some(100),
            last_poll_at: Some(90),
            error: None,
        }
    }

    fn unavailable(name: &str) -> EndpointResult {
        endpoint_error(name.into(), None, "connection refused".into())
    }

    fn decision(now: u64, degraded: bool) -> AggregateDecision {
        let mut endpoints = vec![
            endpoint("a", MonitorAction::FundingOpen),
            endpoint("b", MonitorAction::FundingOpen),
            endpoint("c", MonitorAction::FundingOpen),
        ];
        if degraded {
            endpoints[2] = unavailable("c");
        }
        aggregate_results(&"42".repeat(32), 60, now, endpoints)
    }

    fn test_token_hash() -> Bytes32 {
        sha256_parts(&[b"XHUB_MONITOR_AGGREGATE_ACK_BEARER_V3_6", b"test-token"])
    }

    fn http_state(decision: AggregateDecision, store: AlertStore) -> AggregateHttpState {
        AggregateHttpState {
            funding_coin_id: "42".repeat(32),
            interval_seconds: 60,
            max_staleness_seconds: 180,
            snapshot: Arc::new(RwLock::new(Some(decision))),
            alerts: Arc::new(Mutex::new(store)),
            acknowledgement_token_hash: test_token_hash(),
        }
    }

    fn remote_status() -> RemoteMonitorStatus {
        RemoteMonitorStatus {
            protocol_version: PROTOCOL_VERSION_TEXT.into(),
            status: "READY".into(),
            funding_coin_id: "42".repeat(32),
            rpc_mode: "READ_ONLY".into(),
            fresh: true,
            action: Some(MonitorAction::FundingOpen),
            alert_level: "NONE".into(),
            operator_attention_required: false,
            peak_height: Some(100),
            last_poll_at: Some(90),
            manual_approval_required: true,
            execution_gate: EXECUTION_GATE.into(),
            spend_bundle_created: false,
            broadcast_enabled: false,
            broadcast_ready: false,
            chain_broadcast: false,
        }
    }

    #[test]
    fn endpoints_are_loopback_only_and_distinct() {
        assert!(parse_endpoint("http://127.0.0.1:18741").is_ok());
        assert!(parse_endpoint("http://43.156.47.252:18741").is_err());
        assert!(parse_endpoint("https://127.0.0.1:18741").is_err());
        assert!(parse_endpoint("http://user:pass@127.0.0.1:18741").is_err());
        assert!(parse_listen("0.0.0.0:18744").is_err());
    }

    #[test]
    fn three_of_three_open_is_ready_without_alert() {
        let decision = aggregate_results(
            &"42".repeat(32),
            60,
            100,
            vec![
                endpoint("a", MonitorAction::FundingOpen),
                endpoint("b", MonitorAction::FundingOpen),
                endpoint("c", MonitorAction::FundingOpen),
            ],
        );
        assert_eq!(decision.status, "READY");
        assert_eq!(decision.agreeing_count, 3);
        assert!(decision.quorum_reached);
        assert_eq!(decision.alert_level, "NONE");
        assert!(!decision.operator_attention_required);
        assert!(!decision.broadcast_enabled);
    }

    #[test]
    fn two_of_three_is_degraded_and_chain_transition_is_critical() {
        let degraded = aggregate_results(
            &"42".repeat(32),
            60,
            100,
            vec![
                endpoint("a", MonitorAction::FundingOpen),
                endpoint("b", MonitorAction::FundingOpen),
                unavailable("c"),
            ],
        );
        assert_eq!(degraded.status, "DEGRADED");
        assert_eq!(degraded.alert_level, "WARNING");
        assert!(degraded.operator_attention_required);

        let challenge = aggregate_results(
            &"42".repeat(32),
            60,
            100,
            vec![
                endpoint("a", MonitorAction::ChallengePlanned),
                endpoint("b", MonitorAction::ChallengePlanned),
                endpoint("c", MonitorAction::ChallengePlanned),
            ],
        );
        assert_eq!(challenge.status, "READY");
        assert_eq!(challenge.alert_level, "CRITICAL");
        assert!(challenge.operator_attention_required);
        assert!(!challenge.spend_bundle_created);
        assert!(!challenge.chain_broadcast);
    }

    #[test]
    fn upstream_execution_or_broadcast_capability_is_ineligible() {
        let mut status = remote_status();
        assert!(validate_remote_status(&status, &"42".repeat(32)).is_ok());
        status.broadcast_ready = true;
        assert_eq!(
            validate_remote_status(&status, &"42".repeat(32)).unwrap_err(),
            "monitor reported a prohibited execution or broadcast capability"
        );
    }

    #[test]
    fn alert_store_deduplicates_transitions_and_resolves_previous_alert() {
        let mut store = AlertStore::open_in_memory().expect("store");
        let (normal, created) = store.record(&decision(100, false)).expect("normal");
        assert!(created);

        let mut repeated = decision(101, false);
        repeated.endpoints[0].peak_height = Some(101);
        let (same, created) = store.record(&repeated).expect("repeat");
        assert!(!created);
        assert_eq!(same.event_id, normal.event_id);
        assert_eq!(same.occurrence_count, 2);
        assert_eq!(same.last_seen, 101);

        let (degraded, created) = store.record(&decision(102, true)).expect("degraded");
        assert!(created);
        assert_ne!(degraded.event_id, normal.event_id);
        assert!(degraded.operator_attention_required);

        let (repeated_degraded, created) = store.record(&decision(103, true)).expect("repeat");
        assert!(!created);
        assert_eq!(repeated_degraded.event_id, degraded.event_id);
        assert_eq!(repeated_degraded.occurrence_count, 2);

        let (recovered, created) = store.record(&decision(104, false)).expect("recovered");
        assert!(created);
        assert_ne!(recovered.event_id, degraded.event_id);
        assert_eq!(
            store
                .event(degraded.event_id)
                .expect("resolved")
                .resolved_at,
            Some(104)
        );
        assert_eq!(store.events(10).expect("events").len(), 3);
    }

    #[test]
    fn alert_acknowledgement_is_guarded_and_idempotent() {
        let mut store = AlertStore::open_in_memory().expect("store");
        let (normal, _) = store.record(&decision(100, false)).expect("normal");
        assert!(
            store
                .acknowledge(normal.event_id, "operator-a", "reviewed", 101)
                .is_err()
        );

        let (degraded, _) = store.record(&decision(102, true)).expect("degraded");
        let acknowledged = store
            .acknowledge(degraded.event_id, "operator-a", "reviewed", 103)
            .expect("acknowledge");
        assert_eq!(acknowledged.acknowledged_at, Some(103));
        assert_eq!(acknowledged.acknowledged_by.as_deref(), Some("operator-a"));

        let idempotent = store
            .acknowledge(degraded.event_id, "operator-a", "reviewed", 104)
            .expect("idempotent");
        assert_eq!(idempotent.acknowledged_at, Some(103));
        assert!(
            store
                .acknowledge(degraded.event_id, "operator-b", "different", 104)
                .is_err()
        );

        store.record(&decision(105, false)).expect("recovered");
        assert!(
            store
                .acknowledge(degraded.event_id, "operator-a", "reviewed", 106)
                .is_err()
        );
    }

    #[tokio::test]
    async fn aggregate_http_is_versioned_and_reports_quorum() {
        let now = unix_now().expect("now");
        let decision = aggregate_results(
            &"42".repeat(32),
            60,
            now,
            vec![
                endpoint("a", MonitorAction::FundingOpen),
                endpoint("b", MonitorAction::FundingOpen),
                endpoint("c", MonitorAction::FundingOpen),
            ],
        );
        let state = http_state(decision, AlertStore::open_in_memory().expect("alert store"));
        let app = aggregate_router(state);
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v3.6/monitor-aggregate?protocol_version=0x0360")
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
        assert_eq!(body["quorum_reached"], true);
        assert_eq!(body["agreeing_count"], 3);
        assert_eq!(body["production_ready"], false);
        assert_eq!(body["chain_broadcast"], false);

        let wrong_version = app
            .oneshot(
                Request::get("/api/v3.6/monitor-aggregate?protocol_version=0x9999")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(wrong_version.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn alert_http_requires_token_only_for_acknowledgement() {
        let now = unix_now().expect("now");
        let degraded = decision(now, true);
        let mut store = AlertStore::open_in_memory().expect("alert store");
        let (event, _) = store.record(&degraded).expect("event");
        let app = aggregate_router(http_state(degraded, store));

        let list = app
            .clone()
            .oneshot(
                Request::get("/api/v3.6/alerts?protocol_version=0x0360")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = to_bytes(list.into_body(), usize::MAX).await.expect("body");
        let list_body: serde_json::Value = serde_json::from_slice(&list_body).expect("json");
        assert_eq!(list_body["events"][0]["event_id"], event.event_id);
        assert_eq!(list_body["broadcast_enabled"], false);
        assert_eq!(list_body["chain_broadcast"], false);

        let body = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION_TEXT,
            "operator_id": "operator-a",
            "note": "reviewed"
        })
        .to_string();
        for authorization in [None, Some("Bearer wrong-token")] {
            let mut request =
                Request::post(format!("/api/v3.6/alerts/{}/acknowledge", event.event_id))
                    .header("content-type", "application/json");
            if let Some(value) = authorization {
                request = request.header("authorization", value);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.clone())).expect("request"))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let wrong_version = serde_json::json!({
            "protocol_version": "0x9999",
            "operator_id": "operator-a",
            "note": "reviewed"
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/v3.6/alerts/{}/acknowledge", event.event_id))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(wrong_version))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(
                Request::post(format!("/api/v3.6/alerts/{}/acknowledge", event.event_id))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let response_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let response_body: serde_json::Value =
            serde_json::from_slice(&response_body).expect("json");
        assert_eq!(response_body["status"], "ACKNOWLEDGED");
        assert_eq!(response_body["event"]["acknowledged_by"], "operator-a");
        assert_eq!(response_body["spend_bundle_created"], false);
        assert_eq!(response_body["broadcast_ready"], false);
    }
}
