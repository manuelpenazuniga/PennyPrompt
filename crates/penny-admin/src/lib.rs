//! Admin plane APIs and reporting endpoints.

use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    time::{Duration, Instant},
};

use async_stream::stream;
use axum::{
    extract::{Query, State},
    response::{
        sse::{Event as SseEvent, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use penny_store::{BudgetRepo, SqliteStore, StoreError};
use penny_types::{Budget, Event, EventType, Money, ScopeType, Severity, WindowType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{query, query_scalar, Row};
use tokio::net::TcpListener;
use tracing::error;

#[derive(Debug, Clone)]
pub struct AdminState {
    store: SqliteStore,
    started_at: Instant,
    event_poll_interval: Duration,
    event_batch_size: u32,
}

impl AdminState {
    pub fn new(store: SqliteStore) -> Self {
        Self {
            store,
            started_at: Instant::now(),
            event_poll_interval: Duration::from_millis(500),
            event_batch_size: 100,
        }
    }

    pub fn with_event_poll_interval(mut self, interval: Duration) -> Self {
        self.event_poll_interval = interval.max(Duration::from_millis(100));
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("address parse error: {0}")]
    AddressParse(#[from] std::net::AddrParseError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Serialize)]
struct ApiErrorDetail {
    code: &'static str,
    message: String,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ReportSummaryBy {
    Project,
    Model,
    Session,
}

impl ReportSummaryBy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Model => "model",
            Self::Session => "session",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReportSummaryQuery {
    by: Option<ReportSummaryBy>,
    project: Option<String>,
    model: Option<String>,
    session: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct SummaryRow {
    key: String,
    request_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: Money,
}

#[derive(Debug, Serialize)]
struct SummaryTotals {
    request_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: Money,
}

#[derive(Debug, Serialize)]
struct ReportSummaryResponse {
    by: String,
    rows: Vec<SummaryRow>,
    totals: SummaryTotals,
}

#[derive(Debug, Deserialize)]
struct ReportTopQuery {
    project: Option<String>,
    model: Option<String>,
    session: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct TopRow {
    request_id: String,
    project_id: String,
    session_id: Option<String>,
    model: String,
    provider_id: String,
    status: String,
    started_at: String,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: Money,
    source: String,
}

#[derive(Debug, Deserialize)]
struct UpsertBudgetPayload {
    id: Option<i64>,
    scope_type: ScopeType,
    scope_id: String,
    window_type: WindowType,
    hard_limit_usd: Option<Money>,
    soft_limit_usd: Option<Money>,
    action_on_hard: Option<String>,
    action_on_soft: Option<String>,
    preset_source: Option<String>,
}

#[derive(Debug, Serialize)]
struct BudgetStatusRow {
    budget: Budget,
    accumulated_usd: Money,
    remaining_hard_usd: Option<Money>,
    remaining_soft_usd: Option<Money>,
    hard_ratio: Option<f64>,
    soft_ratio: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    since_id: Option<i64>,
    poll_ms: Option<u64>,
    limit: Option<u32>,
    once: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AdminHealth {
    status: &'static str,
    uptime_seconds: u64,
    db: HealthProbe,
    providers: Vec<String>,
    pricebook: PricebookHealth,
}

#[derive(Debug, Serialize)]
struct HealthProbe {
    status: &'static str,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct PricebookHealth {
    latest_effective_from: Option<String>,
}

pub fn build_router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/health", get(get_health))
        .route("/admin/report/summary", get(get_report_summary))
        .route("/admin/report/top", get(get_report_top))
        .route("/admin/budgets", get(get_budgets).post(post_budgets))
        .route("/admin/events", get(get_events))
        .with_state(state)
}

pub async fn serve(bind: &str, state: AdminState) -> Result<(), AdminError> {
    let addr: SocketAddr = bind.parse()?;
    let listener = TcpListener::bind(addr).await?;
    let app = build_router(state);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_health(State(state): State<AdminState>) -> Response {
    let db_probe = query("SELECT 1").fetch_one(state.store.pool()).await;
    let (db_status, db_error, status_code) = match db_probe {
        Ok(_) => ("up", None, axum::http::StatusCode::OK),
        Err(error) => (
            "down",
            Some(error.to_string()),
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
        ),
    };

    let providers =
        match query_scalar::<_, String>("SELECT id FROM providers WHERE enabled = 1 ORDER BY id")
            .fetch_all(state.store.pool())
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                log_internal_error("admin_health_provider_query_failed", error.to_string());
                Vec::new()
            }
        };
    let latest_effective_from =
        query_scalar::<_, Option<String>>("SELECT MAX(effective_from) FROM pricebook_entries")
            .fetch_one(state.store.pool())
            .await
            .unwrap_or(None);

    (
        status_code,
        Json(AdminHealth {
            status: if status_code == axum::http::StatusCode::OK {
                "ok"
            } else {
                "degraded"
            },
            uptime_seconds: state.started_at.elapsed().as_secs(),
            db: HealthProbe {
                status: db_status,
                error: db_error,
            },
            providers,
            pricebook: PricebookHealth {
                latest_effective_from,
            },
        }),
    )
        .into_response()
}

async fn get_report_summary(
    State(state): State<AdminState>,
    Query(query_params): Query<ReportSummaryQuery>,
) -> Response {
    let by = query_params.by.unwrap_or(ReportSummaryBy::Project);
    let limit = i64::from(query_params.limit.unwrap_or(100).clamp(1, 1_000));
    let (group_expr, join_projects) = match by {
        ReportSummaryBy::Project => (
            "COALESCE(projects.name, requests.project_id)",
            "LEFT JOIN projects ON projects.id = requests.project_id",
        ),
        ReportSummaryBy::Model => ("requests.model_used", ""),
        ReportSummaryBy::Session => ("COALESCE(requests.session_id, '(none)')", ""),
    };

    let sql = format!(
        r#"
        SELECT
            {group_expr} AS group_key,
            COUNT(requests.id) AS request_count,
            CAST(COALESCE(SUM(request_usage.input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(request_usage.output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(request_usage.cost_micros), 0) AS INTEGER) AS total_cost_micros
        FROM requests
        JOIN request_usage ON request_usage.request_id = requests.id
        {join_projects}
        WHERE (?1 IS NULL OR requests.project_id = ?1)
          AND (?2 IS NULL OR requests.model_used = ?2)
          AND (?3 IS NULL OR requests.session_id = ?3)
          AND (?4 IS NULL OR datetime(requests.started_at) >= datetime(?4))
          AND (?5 IS NULL OR datetime(requests.started_at) <= datetime(?5))
        GROUP BY group_key
        ORDER BY total_cost_micros DESC, group_key ASC
        LIMIT ?6
        "#
    );

    let rows = match query(&sql)
        .bind(query_params.project)
        .bind(query_params.model)
        .bind(query_params.session)
        .bind(query_params.since)
        .bind(query_params.until)
        .bind(limit)
        .fetch_all(state.store.pool())
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "report_query_failed",
                error.to_string(),
            )
        }
    };

    let summary_rows = rows
        .into_iter()
        .map(|row| SummaryRow {
            key: row.get("group_key"),
            request_count: row.get("request_count"),
            input_tokens: row.get("input_tokens"),
            output_tokens: row.get("output_tokens"),
            cost_usd: Money::from_micros(row.get("total_cost_micros")),
        })
        .collect::<Vec<_>>();

    let totals = SummaryTotals {
        request_count: summary_rows.iter().map(|row| row.request_count).sum(),
        input_tokens: summary_rows.iter().map(|row| row.input_tokens).sum(),
        output_tokens: summary_rows.iter().map(|row| row.output_tokens).sum(),
        cost_usd: Money::from_micros(
            summary_rows
                .iter()
                .map(|row| row.cost_usd.micros())
                .sum::<i64>(),
        ),
    };

    Json(ReportSummaryResponse {
        by: by.as_str().to_string(),
        rows: summary_rows,
        totals,
    })
    .into_response()
}

async fn get_report_top(
    State(state): State<AdminState>,
    Query(query_params): Query<ReportTopQuery>,
) -> Response {
    let limit = i64::from(query_params.limit.unwrap_or(20).clamp(1, 500));
    let rows = match query(
        r#"
        SELECT
            requests.id AS request_id,
            requests.project_id,
            requests.session_id,
            requests.model_used,
            requests.provider_id,
            requests.status,
            requests.started_at,
            request_usage.input_tokens,
            request_usage.output_tokens,
            request_usage.cost_micros,
            request_usage.source
        FROM requests
        JOIN request_usage ON request_usage.request_id = requests.id
        WHERE (?1 IS NULL OR requests.project_id = ?1)
          AND (?2 IS NULL OR requests.model_used = ?2)
          AND (?3 IS NULL OR requests.session_id = ?3)
          AND (?4 IS NULL OR datetime(requests.started_at) >= datetime(?4))
          AND (?5 IS NULL OR datetime(requests.started_at) <= datetime(?5))
        ORDER BY request_usage.cost_micros DESC, requests.started_at DESC
        LIMIT ?6
        "#,
    )
    .bind(query_params.project)
    .bind(query_params.model)
    .bind(query_params.session)
    .bind(query_params.since)
    .bind(query_params.until)
    .bind(limit)
    .fetch_all(state.store.pool())
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "report_top_query_failed",
                error.to_string(),
            )
        }
    };

    let result = rows
        .into_iter()
        .map(|row| TopRow {
            request_id: row.get("request_id"),
            project_id: row.get("project_id"),
            session_id: row.get("session_id"),
            model: row.get("model_used"),
            provider_id: row.get("provider_id"),
            status: row.get("status"),
            started_at: row.get("started_at"),
            input_tokens: row.get("input_tokens"),
            output_tokens: row.get("output_tokens"),
            cost_usd: Money::from_micros(row.get("cost_micros")),
            source: row.get("source"),
        })
        .collect::<Vec<_>>();

    Json(json!({ "rows": result })).into_response()
}

async fn get_budgets(State(state): State<AdminState>) -> Response {
    let budgets = match state.store.list_all().await {
        Ok(budgets) => budgets,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "budget_query_failed",
                error.to_string(),
            )
        }
    };

    let totals_rows = query(
        r#"
        SELECT ledger.budget_id, ledger.running_total_micros
        FROM cost_ledger AS ledger
        JOIN (
            SELECT budget_id, MAX(id) AS latest_id
            FROM cost_ledger
            GROUP BY budget_id
        ) AS latest ON latest.latest_id = ledger.id
        "#,
    )
    .fetch_all(state.store.pool())
    .await
    .unwrap_or_default();

    let totals = totals_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<i64, _>("budget_id"),
                Money::from_micros(row.get("running_total_micros")),
            )
        })
        .collect::<HashMap<_, _>>();

    let rows = budgets
        .into_iter()
        .map(|budget| {
            let accumulated = totals.get(&budget.id).copied().unwrap_or(Money::ZERO);
            let remaining_hard = budget
                .hard_limit_usd
                .and_then(|limit| limit.checked_sub(accumulated));
            let remaining_soft = budget
                .soft_limit_usd
                .and_then(|limit| limit.checked_sub(accumulated));
            BudgetStatusRow {
                hard_ratio: ratio(accumulated, budget.hard_limit_usd),
                soft_ratio: ratio(accumulated, budget.soft_limit_usd),
                budget,
                accumulated_usd: accumulated,
                remaining_hard_usd: remaining_hard,
                remaining_soft_usd: remaining_soft,
            }
        })
        .collect::<Vec<_>>();

    Json(json!({ "rows": rows })).into_response()
}

async fn post_budgets(
    State(state): State<AdminState>,
    Json(payload): Json<UpsertBudgetPayload>,
) -> Response {
    let budget = Budget {
        id: payload.id.unwrap_or(0),
        scope_type: payload.scope_type,
        scope_id: payload.scope_id,
        window_type: payload.window_type,
        hard_limit_usd: payload.hard_limit_usd,
        soft_limit_usd: payload.soft_limit_usd,
        action_on_hard: payload
            .action_on_hard
            .unwrap_or_else(|| "block".to_string()),
        action_on_soft: payload.action_on_soft.unwrap_or_else(|| "warn".to_string()),
        preset_source: payload.preset_source,
    };

    let stored = match state.store.upsert(&budget).await {
        Ok(stored) => stored,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "budget_upsert_failed",
                error.to_string(),
            )
        }
    };
    Json(stored).into_response()
}

async fn get_events(
    State(state): State<AdminState>,
    Query(query_params): Query<EventsQuery>,
) -> impl IntoResponse {
    let poll_ms = query_params
        .poll_ms
        .unwrap_or(state.event_poll_interval.as_millis() as u64);
    let poll = Duration::from_millis(poll_ms.max(100));
    let batch_size = query_params
        .limit
        .unwrap_or(state.event_batch_size)
        .clamp(1, 500);
    let mut last_id = query_params.since_id.unwrap_or(0);
    let once = query_params.once.unwrap_or(false);
    let store = state.store.clone();

    let stream = stream! {
        let mut ticker = tokio::time::interval(poll);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match fetch_events_since(&store, last_id, batch_size).await {
                Ok(events) => {
                    if events.is_empty() {
                        yield Ok::<SseEvent, Infallible>(SseEvent::default().event("heartbeat").data("{}"));
                        if once {
                            break;
                        }
                        continue;
                    }

                    for event in events {
                        last_id = last_id.max(event.id);
                        let payload = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                        yield Ok::<SseEvent, Infallible>(
                            SseEvent::default()
                                .id(event.id.to_string())
                                .event("event")
                                .data(payload),
                        );
                    }
                    if once {
                        break;
                    }
                }
                Err(error) => {
                    log_internal_error("admin_events_query_failed", error.to_string());
                    let payload = json!({
                        "code": "events_query_failed",
                        "message": error.to_string(),
                    })
                    .to_string();
                    yield Ok::<SseEvent, Infallible>(SseEvent::default().event("error").data(payload));
                    if once {
                        break;
                    }
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::default()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

async fn fetch_events_since(
    store: &SqliteStore,
    since_id: i64,
    limit: u32,
) -> Result<Vec<Event>, StoreError> {
    let rows = query(
        r#"
        SELECT id, request_id, session_id, event_type, severity, detail, created_at
        FROM events
        WHERE id > ?1
        ORDER BY id ASC
        LIMIT ?2
        "#,
    )
    .bind(since_id)
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await?;

    rows.into_iter().map(event_from_row).collect()
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> Result<Event, StoreError> {
    let event_type: String = row.get("event_type");
    let severity: String = row.get("severity");
    let detail: String = row.get("detail");
    Ok(Event {
        id: row.get("id"),
        request_id: row.get("request_id"),
        session_id: row.get("session_id"),
        event_type: event_type_from_db(&event_type)?,
        severity: severity_from_db(&severity)?,
        detail: serde_json::from_str(&detail).unwrap_or_else(|_| json!({"raw": detail})),
        created_at: parse_db_datetime(row.get("created_at"), "events.created_at")?,
    })
}

fn event_type_from_db(value: &str) -> Result<EventType, StoreError> {
    match value {
        "budget_check" => Ok(EventType::BudgetCheck),
        "budget_block" => Ok(EventType::BudgetBlock),
        "budget_warn" => Ok(EventType::BudgetWarn),
        "reserve" => Ok(EventType::Reserve),
        "reconcile" => Ok(EventType::Reconcile),
        "release" => Ok(EventType::Release),
        "loop_detected" => Ok(EventType::LoopDetected),
        "burn_rate_alert" => Ok(EventType::BurnRateAlert),
        "session_paused" => Ok(EventType::SessionPaused),
        "provider_failure" => Ok(EventType::ProviderFailure),
        "mode_failsafe" => Ok(EventType::ModeFailsafe),
        _ => Err(StoreError::InvalidEnum {
            field: "events.event_type",
            value: value.to_string(),
        }),
    }
}

fn severity_from_db(value: &str) -> Result<Severity, StoreError> {
    match value {
        "info" => Ok(Severity::Info),
        "warn" => Ok(Severity::Warn),
        "error" => Ok(Severity::Error),
        "critical" => Ok(Severity::Critical),
        _ => Err(StoreError::InvalidEnum {
            field: "events.severity",
            value: value.to_string(),
        }),
    }
}

fn parse_db_datetime(value: String, field: &'static str) -> Result<DateTime<Utc>, StoreError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(&value) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    Err(StoreError::InvalidTimestamp { field, value })
}

fn ratio(accumulated: Money, limit: Option<Money>) -> Option<f64> {
    let limit = limit?;
    if limit.micros() == 0 {
        return None;
    }
    Some(accumulated.to_usd() / limit.to_usd())
}

fn api_error(status: axum::http::StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(ApiError {
            error: ApiErrorDetail { code, message },
        }),
    )
        .into_response()
}

fn log_internal_error(tag: &str, detail: String) {
    error!(tag = tag, detail = %detail, "internal admin error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use penny_proxy::{build_router as build_proxy_router, ProxyState};
    use penny_store::{
        BudgetRepo, EventRepo, NewEvent, NewRequest, ProjectRepo, RequestRepo, SessionRepo,
        UsageRecord,
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;

    async fn setup_store() -> SqliteStore {
        SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store")
    }

    async fn seed_minimal_data(store: &SqliteStore) {
        let project_id = store
            .upsert_by_path("/tmp/pennyprompt-admin")
            .await
            .expect("upsert project");
        let session_id = store.create(&project_id).await.expect("create session");
        let request = NewRequest {
            id: "req_admin_01".to_string(),
            session_id: Some(session_id.clone()),
            project_id: project_id.clone(),
            model_requested: "gpt-4.1".to_string(),
            model_used: "gpt-4.1".to_string(),
            provider_id: "openai".to_string(),
            started_at: Utc::now(),
            is_streaming: false,
        };
        RequestRepo::insert(store, &request)
            .await
            .expect("insert request");
        store
            .insert_usage(&UsageRecord {
                request_id: request.id.clone(),
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: Money::from_usd(2.5).expect("money"),
                source: penny_types::UsageSource::Provider,
                pricing_snapshot: json!({"test": true}),
            })
            .await
            .expect("insert usage");

        let budget = Budget {
            id: 0,
            scope_type: ScopeType::Global,
            scope_id: "*".to_string(),
            window_type: WindowType::Day,
            hard_limit_usd: Some(Money::from_usd(10.0).expect("money")),
            soft_limit_usd: Some(Money::from_usd(7.0).expect("money")),
            action_on_hard: "block".to_string(),
            action_on_soft: "warn".to_string(),
            preset_source: None,
        };
        let stored_budget = store.upsert(&budget).await.expect("upsert budget");
        query(
            r#"
            INSERT INTO cost_ledger (request_id, entry_type, budget_id, amount_usd, running_total, amount_micros, running_total_micros)
            VALUES (?1, 'reconcile', ?2, 2.5, 2.5, 2500000, 2500000)
            "#,
        )
        .bind("req_admin_01")
        .bind(stored_budget.id)
        .execute(store.pool())
        .await
        .expect("insert ledger");

        EventRepo::insert(
            store,
            &NewEvent {
                request_id: Some("req_admin_01".to_string()),
                session_id: Some(session_id),
                event_type: EventType::Reconcile,
                severity: Severity::Info,
                detail: json!({"source":"test"}),
            },
        )
        .await
        .expect("insert event");
    }

    #[tokio::test]
    async fn admin_health_returns_ok_with_db_probe() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json_body["db"]["status"], "up");
    }

    #[tokio::test]
    async fn report_summary_matches_usage_rows_in_store() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/report/summary?by=project")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json_body["totals"]["request_count"], 1);
        assert_eq!(json_body["totals"]["input_tokens"], 100);
        assert_eq!(json_body["totals"]["output_tokens"], 50);
        assert_eq!(json_body["totals"]["cost_usd"], 2.5);
    }

    #[tokio::test]
    async fn budgets_endpoint_reflects_budget_and_running_total() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/budgets")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json");
        let rows = json_body["rows"].as_array().expect("rows");
        assert!(!rows.is_empty());
        assert_eq!(rows[0]["accumulated_usd"], 2.5);
        assert_eq!(rows[0]["remaining_hard_usd"], 7.5);
    }

    #[tokio::test]
    async fn budgets_post_upserts_runtime_budget() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        let app = build_router(AdminState::new(store.clone()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/budgets")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "scope_type": "project",
                            "scope_id": "project-alpha",
                            "window_type": "week",
                            "hard_limit_usd": 55.0,
                            "action_on_hard": "block",
                            "action_on_soft": "warn"
                        })
                        .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let budgets = store.list_all().await.expect("list budgets");
        assert!(budgets.iter().any(|budget| {
            budget.scope_type == ScopeType::Project
                && budget.scope_id == "project-alpha"
                && budget.window_type == WindowType::Week
                && budget.hard_limit_usd == Some(Money::from_usd(55.0).expect("money"))
        }));
    }

    #[tokio::test]
    async fn events_endpoint_streams_sse_payload() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/events?once=true&poll_ms=100")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let text = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(text.contains("event: event") || text.contains("event: heartbeat"));
    }

    #[tokio::test]
    async fn admin_endpoints_are_not_exposed_on_proxy_router() {
        let proxy_app = build_proxy_router(ProxyState::mock_default());
        let response = proxy_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
