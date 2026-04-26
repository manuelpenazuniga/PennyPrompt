//! Admin plane APIs and reporting endpoints.

use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use async_stream::stream;
use axum::{
    extract::{Query, State},
    response::{
        sse::{Event as SseEvent, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use penny_config::LoopAction;
use penny_cost::{estimate_tokens, PricingEngine};
use penny_detect::{DetectEngine, DetectStatus};
use penny_store::{BudgetRepo, EventRepo, NewEvent, SqliteStore, StoreError};
use penny_types::{
    Budget, CostRange, Event, EventType, Money, ScopeType, Severity, TaskType, WindowType,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{query, query_scalar, Row};
use tokio::net::TcpListener;
use tracing::error;

#[derive(Debug, Clone)]
pub struct AdminState {
    store: SqliteStore,
    detector: Arc<DetectEngine>,
    started_at: Instant,
    event_poll_interval: Duration,
    event_batch_size: u32,
}

impl AdminState {
    pub fn new(store: SqliteStore) -> Self {
        Self {
            store,
            detector: Arc::new(DetectEngine::new(penny_detect::DetectorConfig {
                enabled: false,
                burn_rate_alert_usd_per_hour: 10.0,
                loop_window_seconds: 120,
                loop_threshold_similar_requests: 8,
                loop_action: LoopAction::Alert,
                min_burn_rate_observation_seconds: 30,
                max_recorded_events: 5000,
                session_state_retention_seconds: 3600,
                max_sessions: 2048,
            })),
            started_at: Instant::now(),
            event_poll_interval: Duration::from_millis(500),
            event_batch_size: 100,
        }
    }

    pub fn with_detector(mut self, detector: Arc<DetectEngine>) -> Self {
        self.detector = detector;
        self
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
    #[error("invalid bind target `{0}`")]
    InvalidBind(String),
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
    page_totals: SummaryTotals,
    total_groups: i64,
    returned_groups: i64,
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
struct DetectStatusResponse {
    enabled: bool,
    paused_sessions: Vec<penny_detect::PausedSession>,
    active_alerts: Vec<penny_detect::SessionAlert>,
}

#[derive(Debug, Deserialize)]
struct DetectResumePayload {
    session_id: String,
    request_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct DetectResumeResponse {
    resumed: bool,
    session_id: String,
    event: penny_detect::DetectEventRecord,
    persisted: bool,
    consistency: DetectResumeConsistency,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<DetectResumeWarning>,
}

#[derive(Debug, Serialize)]
struct DetectResumeConsistency {
    mode: &'static str,
    event_persistence_guarantee: &'static str,
    event_persisted: bool,
}

#[derive(Debug, Serialize)]
struct DetectResumeWarning {
    code: &'static str,
    message: String,
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

#[derive(Debug, Deserialize)]
struct EstimateRequest {
    model: String,
    #[serde(default = "default_task_type")]
    task_type: TaskType,
    context_tokens: Option<u64>,
    messages: Option<Value>,
    project_id: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct EstimateResponse {
    model: String,
    task_type: TaskType,
    context_tokens: u64,
    range: CostRange,
    pricebook_snapshot: Value,
    route_preview: Option<RoutePreview>,
    budget: EstimateBudgetSummary,
}

#[derive(Debug, Serialize)]
struct RoutePreview {
    provider_id: String,
    provider_name: String,
    api_format: String,
    external_model: String,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct EstimateBudgetSummary {
    status: String,
    max_estimated_cost_usd: Money,
    rows: Vec<EstimateBudgetRow>,
}

#[derive(Debug, Serialize)]
struct EstimateBudgetRow {
    budget_id: i64,
    scope_type: ScopeType,
    scope_id: String,
    window_type: WindowType,
    accumulated_usd: Money,
    projected_usd: Money,
    hard_limit_usd: Option<Money>,
    soft_limit_usd: Option<Money>,
    hard_limit_exceeded: bool,
    soft_limit_reached: bool,
}

pub fn build_router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/health", get(get_health))
        .route("/admin/report/summary", get(get_report_summary))
        .route("/admin/report/top", get(get_report_top))
        .route("/admin/budgets", get(get_budgets).post(post_budgets))
        .route("/admin/estimate", post(post_estimate))
        .route("/admin/detect/status", get(get_detect_status))
        .route("/admin/detect/resume", post(post_detect_resume))
        .route("/admin/events", get(get_events))
        .with_state(state)
}

pub async fn serve(bind: &str, state: AdminState) -> Result<(), AdminError> {
    serve_with_shutdown(bind, state, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown<F>(
    bind: &str,
    state: AdminState,
    shutdown: F,
) -> Result<(), AdminError>
where
    F: Future<Output = ()> + Send + 'static,
{
    if let Ok(addr) = bind.parse::<SocketAddr>() {
        let listener = TcpListener::bind(addr).await?;
        let app = build_router(state);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await?;
        return Ok(());
    }

    #[cfg(unix)]
    {
        let socket_path = normalize_unix_socket_path(bind)?;
        let listener = bind_unix_listener(&socket_path)?;
        let app = build_router(state);
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await;
        cleanup_unix_socket(&socket_path)?;
        result?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (state, shutdown);
        Err(AdminError::InvalidBind(bind.to_string()))
    }
}

#[cfg(unix)]
fn normalize_unix_socket_path(raw: &str) -> Result<PathBuf, AdminError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AdminError::InvalidBind(raw.to_string()));
    }
    Ok(PathBuf::from(trimmed))
}

#[cfg(unix)]
fn bind_unix_listener(path: &Path) -> Result<tokio::net::UnixListener, AdminError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    cleanup_unix_socket(path)?;
    Ok(tokio::net::UnixListener::bind(path)?)
}

#[cfg(unix)]
fn cleanup_unix_socket(path: &Path) -> Result<(), AdminError> {
    match std::fs::remove_file(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AdminError::Io(error)),
    }
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
            .await;
    let latest_effective_from = match latest_effective_from {
        Ok(value) => value,
        Err(error) => {
            log_internal_error("admin_health_pricebook_query_failed", error.to_string());
            None
        }
    };

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
    let project_filter = query_params.project.clone();
    let model_filter = query_params.model.clone();
    let session_filter = query_params.session.clone();
    let since_filter = query_params.since.clone();
    let until_filter = query_params.until.clone();
    let (group_expr, join_projects) = match by {
        ReportSummaryBy::Project => (
            "COALESCE(projects.name, requests.project_id)",
            "LEFT JOIN projects ON projects.id = requests.project_id",
        ),
        ReportSummaryBy::Model => ("requests.model_used", ""),
        ReportSummaryBy::Session => ("COALESCE(requests.session_id, '(none)')", ""),
    };
    // SQL fragments below are selected from trusted enum variants only.
    // All user-provided filters stay parameterized through bind placeholders.

    let grouped_sql = format!(
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
        "#
    );
    let rows_sql = format!(
        r#"
        {grouped_sql}
        ORDER BY total_cost_micros DESC, group_key ASC
        LIMIT ?6
        "#
    );
    let totals_sql = format!(
        r#"
        WITH grouped AS (
            {grouped_sql}
        )
        SELECT
            CAST(COALESCE(SUM(request_count), 0) AS INTEGER) AS request_count,
            CAST(COALESCE(SUM(input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(total_cost_micros), 0) AS INTEGER) AS total_cost_micros,
            CAST(COUNT(*) AS INTEGER) AS total_groups
        FROM grouped
        "#
    );

    let rows = match query(&rows_sql)
        .bind(project_filter.clone())
        .bind(model_filter.clone())
        .bind(session_filter.clone())
        .bind(since_filter.clone())
        .bind(until_filter.clone())
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

    let page_totals = SummaryTotals {
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
    let totals_row = match query(&totals_sql)
        .bind(project_filter)
        .bind(model_filter)
        .bind(session_filter)
        .bind(since_filter)
        .bind(until_filter)
        .fetch_one(state.store.pool())
        .await
    {
        Ok(row) => row,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "report_totals_query_failed",
                error.to_string(),
            )
        }
    };
    let totals = SummaryTotals {
        request_count: totals_row.get::<i64, _>("request_count"),
        input_tokens: totals_row.get::<i64, _>("input_tokens"),
        output_tokens: totals_row.get::<i64, _>("output_tokens"),
        cost_usd: Money::from_micros(totals_row.get::<i64, _>("total_cost_micros")),
    };
    let total_groups = totals_row.get::<i64, _>("total_groups");
    let returned_groups = summary_rows.len() as i64;

    Json(ReportSummaryResponse {
        by: by.as_str().to_string(),
        rows: summary_rows,
        totals,
        page_totals,
        total_groups,
        returned_groups,
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

    let totals = match fetch_latest_running_totals(&state.store).await {
        Ok(totals) => totals,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "budget_running_totals_query_failed",
                error.to_string(),
            )
        }
    };

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

async fn post_estimate(
    State(state): State<AdminState>,
    Json(payload): Json<EstimateRequest>,
) -> Response {
    let model = payload.model.trim();
    if model.is_empty() {
        return api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "estimate_invalid_model",
            "model is required".to_string(),
        );
    }

    let context_tokens = match payload.context_tokens {
        Some(tokens) => tokens,
        None => {
            let Some(messages) = payload.messages.as_ref() else {
                return api_error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "estimate_invalid_request",
                    "provide context_tokens or messages".to_string(),
                );
            };
            estimate_tokens(messages).input_tokens
        }
    };

    let engine = PricingEngine::new(&state.store);
    let range = match engine
        .estimate_range(model, context_tokens, payload.task_type.clone())
        .await
    {
        Ok(range) => range,
        Err(error) => {
            let status = if is_price_not_found(&error.to_string()) {
                axum::http::StatusCode::NOT_FOUND
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            return api_error(status, "estimate_failed", error.to_string());
        }
    };
    let pricebook_snapshot = match engine.snapshot(model).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let status = if is_price_not_found(&error.to_string()) {
                axum::http::StatusCode::NOT_FOUND
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            return api_error(status, "pricebook_snapshot_failed", error.to_string());
        }
    };

    let max_estimated_cost_usd = match Money::from_usd(range.max_usd) {
        Ok(value) => value,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "estimate_overflow",
                error.to_string(),
            )
        }
    };

    let budget = match build_estimate_budget_summary(
        &state.store,
        payload.project_id.as_deref(),
        payload.session_id.as_deref(),
        max_estimated_cost_usd,
    )
    .await
    {
        Ok(summary) => summary,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "estimate_budget_summary_failed",
                error.to_string(),
            )
        }
    };

    let route_preview = match fetch_route_preview(&state.store, model).await {
        Ok(preview) => preview,
        Err(error) => {
            return api_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "estimate_route_preview_failed",
                error.to_string(),
            )
        }
    };

    Json(EstimateResponse {
        model: model.to_string(),
        task_type: payload.task_type,
        context_tokens,
        range,
        pricebook_snapshot,
        route_preview,
        budget,
    })
    .into_response()
}

async fn get_detect_status(State(state): State<AdminState>) -> Response {
    let status = state.detector.status();
    Json(detect_status_response(
        state.detector.config().enabled,
        status,
    ))
    .into_response()
}

async fn post_detect_resume(
    State(state): State<AdminState>,
    Json(payload): Json<DetectResumePayload>,
) -> Response {
    let session_id = payload.session_id.trim();
    if session_id.is_empty() {
        return api_error(
            axum::http::StatusCode::BAD_REQUEST,
            "detect_resume_invalid_session",
            "session_id is required".to_string(),
        );
    }

    let Some(event) = state
        .detector
        .resume_session(session_id, payload.request_id.as_deref())
    else {
        return api_error(
            axum::http::StatusCode::NOT_FOUND,
            "detect_session_not_paused",
            format!("session `{session_id}` is not paused"),
        );
    };

    // Consistency policy: best-effort persistence.
    // The resume action is applied in-memory first to unblock the session; event persistence
    // is attempted afterwards and reported explicitly in the response contract.
    let mut persisted = true;
    if let Err(error) = EventRepo::insert(
        &state.store,
        &NewEvent {
            request_id: event.request_id.clone(),
            session_id: Some(event.session_id.clone()),
            event_type: event.event_type.clone(),
            severity: event.severity.clone(),
            detail: event.detail.clone(),
        },
    )
    .await
    {
        persisted = false;
        log_internal_error("detect_resume_persist_failed", error.to_string());
    }

    let warning = (!persisted).then(|| DetectResumeWarning {
        code: "detect_resume_event_not_persisted",
        message: "session resumed in-memory but resume event could not be persisted".to_string(),
    });

    Json(DetectResumeResponse {
        resumed: true,
        session_id: session_id.to_string(),
        event,
        persisted,
        consistency: DetectResumeConsistency {
            mode: "best_effort_resume_then_persist",
            event_persistence_guarantee: "best_effort",
            event_persisted: persisted,
        },
        warning,
    })
    .into_response()
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
                        let payload = serde_json::to_string(&event).unwrap_or_else(|error| {
                            log_internal_error("admin_events_serialize_failed", error.to_string());
                            "{}".to_string()
                        });
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

async fn fetch_latest_running_totals(
    store: &SqliteStore,
) -> Result<HashMap<i64, Money>, sqlx::Error> {
    let rows = query(
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
    .fetch_all(store.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<i64, _>("budget_id"),
                Money::from_micros(row.get("running_total_micros")),
            )
        })
        .collect())
}

async fn build_estimate_budget_summary(
    store: &SqliteStore,
    project_id: Option<&str>,
    session_id: Option<&str>,
    max_estimated_cost_usd: Money,
) -> Result<EstimateBudgetSummary, StoreError> {
    let mut budgets = if let (Some(project_id), Some(session_id)) = (project_id, session_id) {
        store
            .list_applicable_for_request(project_id, session_id)
            .await?
    } else {
        store
            .list_all()
            .await?
            .into_iter()
            .filter(|budget| match budget.scope_type {
                ScopeType::Global => budget.scope_id == "*",
                ScopeType::Project => project_id.is_some_and(|project| project == budget.scope_id),
                ScopeType::Session => session_id.is_some_and(|session| session == budget.scope_id),
            })
            .collect::<Vec<_>>()
    };
    budgets.sort_by_key(|budget| budget.id);

    if budgets.is_empty() {
        return Ok(EstimateBudgetSummary {
            status: "no_budgets".to_string(),
            max_estimated_cost_usd,
            rows: Vec::new(),
        });
    }

    let running_totals = fetch_latest_running_totals(store)
        .await
        .map_err(StoreError::from)?;
    let mut rows = Vec::with_capacity(budgets.len());
    let mut hard_limit_exceeded = false;
    let mut soft_limit_reached = false;

    for budget in budgets {
        let accumulated_usd = running_totals
            .get(&budget.id)
            .copied()
            .unwrap_or(Money::ZERO);
        let projected_usd = accumulated_usd
            .checked_add(max_estimated_cost_usd)
            .unwrap_or(Money::from_micros(i64::MAX));
        let hard_exceeded = budget
            .hard_limit_usd
            .map(|limit| projected_usd > limit)
            .unwrap_or(false);
        let soft_reached = budget
            .soft_limit_usd
            .map(|limit| projected_usd >= limit)
            .unwrap_or(false);

        hard_limit_exceeded |= hard_exceeded;
        soft_limit_reached |= soft_reached;

        rows.push(EstimateBudgetRow {
            budget_id: budget.id,
            scope_type: budget.scope_type,
            scope_id: budget.scope_id,
            window_type: budget.window_type,
            accumulated_usd,
            projected_usd,
            hard_limit_usd: budget.hard_limit_usd,
            soft_limit_usd: budget.soft_limit_usd,
            hard_limit_exceeded: hard_exceeded,
            soft_limit_reached: soft_reached,
        });
    }

    let status = if hard_limit_exceeded {
        "over_hard_limit"
    } else if soft_limit_reached {
        "soft_limit_reached"
    } else {
        "within_limit"
    };

    Ok(EstimateBudgetSummary {
        status: status.to_string(),
        max_estimated_cost_usd,
        rows,
    })
}

async fn fetch_route_preview(
    store: &SqliteStore,
    model_id: &str,
) -> Result<Option<RoutePreview>, sqlx::Error> {
    let row = query(
        r#"
        SELECT
            providers.id AS provider_id,
            providers.name AS provider_name,
            providers.api_format AS api_format,
            providers.enabled AS provider_enabled,
            models.external_name AS external_model
        FROM models
        JOIN providers ON providers.id = models.provider_id
        WHERE models.id = ?1
        LIMIT 1
        "#,
    )
    .bind(model_id)
    .fetch_optional(store.pool())
    .await?;

    Ok(row.map(|row| RoutePreview {
        provider_id: row.get("provider_id"),
        provider_name: row.get("provider_name"),
        api_format: row.get("api_format"),
        external_model: row.get("external_model"),
        enabled: row.get::<i64, _>("provider_enabled") != 0,
    }))
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
        "session_resumed" => Ok(EventType::SessionResumed),
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

fn detect_status_response(enabled: bool, status: DetectStatus) -> DetectStatusResponse {
    DetectStatusResponse {
        enabled,
        paused_sessions: status.paused_sessions,
        active_alerts: status.active_alerts,
    }
}

fn default_task_type() -> TaskType {
    TaskType::SinglePass
}

fn is_price_not_found(message: &str) -> bool {
    message.starts_with("no active pricebook entry found")
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
    use std::sync::Arc;

    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use penny_config::LoopAction;
    use penny_detect::{DetectEngine, DetectorConfig};
    use penny_proxy::{build_router as build_proxy_router, ProxyState};
    use penny_store::{
        BudgetRepo, EventRepo, NewEvent, NewRequest, ProjectRepo, RequestRepo, SessionRepo,
        UsageRecord,
    };
    use penny_types::RequestDigest;
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

    async fn seed_estimate_pricing(store: &SqliteStore, model_id: &str, provider_id: &str) {
        query(
            r#"
            INSERT INTO providers (id, name, base_url, api_format, enabled)
            VALUES (?1, 'Anthropic', 'https://api.anthropic.com', 'anthropic', 1)
            "#,
        )
        .bind(provider_id)
        .execute(store.pool())
        .await
        .expect("insert provider");

        query(
            r#"
            INSERT INTO models (id, provider_id, external_name, display_name, class)
            VALUES (?1, ?2, ?3, ?3, 'balanced')
            "#,
        )
        .bind(model_id)
        .bind(provider_id)
        .bind(model_id)
        .execute(store.pool())
        .await
        .expect("insert model");

        query(
            r#"
            INSERT INTO pricebook_entries (
                model_id,
                input_per_mtok,
                output_per_mtok,
                input_per_mtok_micros,
                output_per_mtok_micros,
                effective_from,
                source
            )
            VALUES (?1, 3.0, 15.0, 3000000, 15000000, datetime('now', '-1 day'), 'test')
            "#,
        )
        .bind(model_id)
        .execute(store.pool())
        .await
        .expect("insert pricebook");
    }

    fn detector_with_paused_session(session_id: &str) -> Arc<DetectEngine> {
        let detector = Arc::new(DetectEngine::new(DetectorConfig {
            enabled: true,
            burn_rate_alert_usd_per_hour: 9999.0,
            loop_window_seconds: 120,
            loop_threshold_similar_requests: 2,
            loop_action: LoopAction::Pause,
            min_burn_rate_observation_seconds: 30,
            max_recorded_events: 5000,
            session_state_retention_seconds: 3600,
            max_sessions: 2048,
        }));
        let now = Utc::now();
        let digest = |timestamp: DateTime<Utc>| RequestDigest {
            model: "claude-sonnet-4-6".to_string(),
            input_tokens: 100,
            cost_usd: Money::from_usd(0.1).expect("money"),
            tool_name: None,
            tool_succeeded: true,
            content_hash: 42,
            timestamp,
        };
        detector.feed(session_id, Some("req-a"), digest(now));
        detector.feed(
            session_id,
            Some("req-b"),
            digest(now + chrono::Duration::seconds(2)),
        );
        detector
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
        assert_eq!(json_body["page_totals"]["request_count"], 1);
        assert_eq!(json_body["returned_groups"], 1);
        assert_eq!(json_body["total_groups"], 1);
    }

    #[tokio::test]
    async fn report_summary_returns_filtered_totals_and_page_totals_separately() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;

        let project_id = store
            .upsert_by_path("/tmp/pennyprompt-admin-2")
            .await
            .expect("upsert project");
        let session_id = store.create(&project_id).await.expect("create session");
        let request = NewRequest {
            id: "req_admin_02".to_string(),
            session_id: Some(session_id),
            project_id,
            model_requested: "gpt-4.1".to_string(),
            model_used: "gpt-4.1".to_string(),
            provider_id: "openai".to_string(),
            started_at: Utc::now(),
            is_streaming: false,
        };
        RequestRepo::insert(&store, &request)
            .await
            .expect("insert request");
        store
            .insert_usage(&UsageRecord {
                request_id: request.id.clone(),
                input_tokens: 200,
                output_tokens: 100,
                cost_usd: Money::from_usd(5.0).expect("money"),
                source: penny_types::UsageSource::Provider,
                pricing_snapshot: json!({"test": true}),
            })
            .await
            .expect("insert usage");

        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/report/summary?by=project&limit=1")
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
        assert_eq!(json_body["total_groups"], 2);
        assert_eq!(json_body["returned_groups"], 1);
        assert_eq!(json_body["totals"]["request_count"], 2);
        assert_eq!(json_body["totals"]["input_tokens"], 300);
        assert_eq!(json_body["totals"]["output_tokens"], 150);
        assert_eq!(json_body["totals"]["cost_usd"], 7.5);
        assert_eq!(json_body["page_totals"]["request_count"], 1);
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
    async fn estimate_endpoint_returns_range_route_and_budget_summary() {
        let store = setup_store().await;
        seed_minimal_data(&store).await;
        seed_estimate_pricing(&store, "claude-sonnet-4-6", "anthropic").await;
        let app = build_router(AdminState::new(store));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/estimate")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "model": "claude-sonnet-4-6",
                            "task_type": "multi_round",
                            "context_tokens": 1200,
                            "project_id": "pennyprompt-admin",
                            "session_id": "not-present"
                        })
                        .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json_body["model"], "claude-sonnet-4-6");
        assert_eq!(json_body["task_type"], "multi_round");
        assert_eq!(json_body["context_tokens"], 1200);
        assert_eq!(json_body["range"]["confidence"], "medium");
        assert_eq!(json_body["route_preview"]["provider_id"], "anthropic");
        assert_eq!(json_body["budget"]["status"], "within_limit");
        assert_eq!(json_body["budget"]["rows"][0]["accumulated_usd"], 2.5);
    }

    #[tokio::test]
    async fn detect_status_lists_paused_sessions_and_active_alerts() {
        let store = setup_store().await;
        let detector = detector_with_paused_session("sess-detect-a");
        let app = build_router(AdminState::new(store).with_detector(detector));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/detect/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["enabled"], true);
        assert_eq!(
            json_body["paused_sessions"][0]["session_id"],
            "sess-detect-a"
        );
        assert_eq!(
            json_body["paused_sessions"][0]["reason"],
            "session_paused_loop_detected"
        );
        assert_eq!(json_body["active_alerts"][0]["session_id"], "sess-detect-a");
    }

    #[tokio::test]
    async fn detect_resume_clears_paused_session_and_persists_event() {
        let store = setup_store().await;
        let detector = detector_with_paused_session("sess-detect-b");
        let app = build_router(AdminState::new(store.clone()).with_detector(detector));

        let resume_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/detect/resume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "session_id": "sess-detect-b",
                            "request_id": "req-resume-cli"
                        })
                        .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("response");

        assert_eq!(resume_response.status(), StatusCode::OK);
        let body = to_bytes(resume_response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let resume_json: Value = serde_json::from_slice(&body).expect("resume json");
        assert_eq!(resume_json["resumed"], true);
        assert_eq!(resume_json["session_id"], "sess-detect-b");
        assert_eq!(resume_json["persisted"], true);
        assert_eq!(
            resume_json["consistency"]["mode"],
            "best_effort_resume_then_persist"
        );
        assert_eq!(
            resume_json["consistency"]["event_persistence_guarantee"],
            "best_effort"
        );
        assert_eq!(resume_json["consistency"]["event_persisted"], true);
        assert!(resume_json["warning"].is_null());

        let status_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/detect/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        let body = to_bytes(status_response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let status_json: Value = serde_json::from_slice(&body).expect("status json");
        assert!(status_json["paused_sessions"]
            .as_array()
            .expect("array")
            .is_empty());

        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM events WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
        )
        .bind("sess-detect-b")
        .fetch_all(store.pool())
        .await
        .expect("event rows");
        assert!(rows.iter().any(|row| row == "session_resumed"));
    }

    #[tokio::test]
    async fn detect_resume_reports_partial_success_when_event_persistence_fails() {
        let store = setup_store().await;
        let detector = detector_with_paused_session("sess-detect-c");
        let app = build_router(AdminState::new(store.clone()).with_detector(detector));

        sqlx::query("DROP TABLE events")
            .execute(store.pool())
            .await
            .expect("drop events table");

        let resume_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/detect/resume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "session_id": "sess-detect-c",
                            "request_id": "req-resume-cli"
                        })
                        .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("response");

        assert_eq!(resume_response.status(), StatusCode::OK);
        let body = to_bytes(resume_response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let resume_json: Value = serde_json::from_slice(&body).expect("resume json");
        assert_eq!(resume_json["resumed"], true);
        assert_eq!(resume_json["persisted"], false);
        assert_eq!(
            resume_json["consistency"]["mode"],
            "best_effort_resume_then_persist"
        );
        assert_eq!(resume_json["consistency"]["event_persisted"], false);
        assert_eq!(
            resume_json["warning"]["code"],
            "detect_resume_event_not_persisted"
        );
        assert!(resume_json["warning"]["message"]
            .as_str()
            .expect("warning message")
            .contains("resumed in-memory"));

        let status_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/detect/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        let body = to_bytes(status_response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let status_json: Value = serde_json::from_slice(&body).expect("status json");
        assert!(status_json["paused_sessions"]
            .as_array()
            .expect("array")
            .is_empty());
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
