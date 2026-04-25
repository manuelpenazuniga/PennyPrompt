//! Proxy plane implementation for PennyPrompt.

use std::{
    convert::Infallible,
    hash::Hasher,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{Json, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Router,
};
use bytes::Bytes;
use chrono::Utc;
use penny_budget::BudgetEvaluator;
use penny_config::{
    AppConfig as RuntimeConfig, BudgetConfig as RuntimeBudgetConfig,
    CleanupConfig as RuntimeCleanupConfig, LoopAction, ScopeType as ConfigScopeType,
    WindowType as ConfigWindowType,
};
use penny_cost::{estimate_tokens, PricingEngine};
use penny_detect::{DetectEngine, DetectEventRecord, DetectorConfig, SESSION_PAUSED_LOOP_REASON};
use penny_ledger::CostLedger;
use penny_providers::{MockProvider, MockProviderConfig, ProviderAdapter, ProviderError};
use penny_store::{
    BudgetRepo, EventRepo, NewEvent, NewRequest, ProjectRepo, RequestRepo, SessionRepo,
    SqliteStore, UsageRecord,
};
use penny_types::{
    Budget, BudgetBlockDetail, EventType, Mode, Money, NormalizedRequest, RequestDigest,
    ResponseBody, RouteDecision, ScopeType, Severity, UsageSource, WindowType,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{query, query_scalar};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;
use uuid::Uuid;

pub const DEFAULT_PROXY_BIND: &str = "127.0.0.1:8585";
const REQUEST_ID_HEADER: &str = "x-penny-request-id";
const PROJECT_OVERRIDE_HEADER: &str = "x-penny-project";
const SESSION_OVERRIDE_HEADER: &str = "x-penny-session";
const CWD_OVERRIDE_HEADER: &str = "x-penny-cwd";
const INTERNAL_HEALTH_HEADER: &str = "x-penny-internal-health";
const INTERNAL_HEALTH_HEADER_VALUE: &str = "1";

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("address parse error: {0}")]
    AddressParse(#[from] std::net::AddrParseError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct ProxyState {
    provider: Arc<dyn ProviderAdapter>,
    detector: Arc<DetectEngine>,
    store: Option<SqliteStore>,
    models: Vec<String>,
    mode: Mode,
    default_project_id: String,
    default_session_id: String,
    session_window_minutes: u64,
    health_db_probe_timeout: Duration,
    cleanup: CleanupSettings,
    started_at: Instant,
}

impl ProxyState {
    pub fn with_provider(provider: Arc<dyn ProviderAdapter>, models: Vec<String>) -> Self {
        Self {
            provider,
            detector: Arc::new(DetectEngine::new(DetectorConfig {
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
            store: None,
            models,
            mode: Mode::Guard,
            default_project_id: "default".to_string(),
            default_session_id: "session-auto".to_string(),
            session_window_minutes: 30,
            health_db_probe_timeout: Duration::from_millis(200),
            cleanup: CleanupSettings::default(),
            started_at: Instant::now(),
        }
    }

    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    pub fn with_detector(mut self, detector: Arc<DetectEngine>) -> Self {
        self.detector = detector;
        self
    }

    pub fn with_session_window_minutes(mut self, minutes: u64) -> Self {
        self.session_window_minutes = minutes.max(1);
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_health_db_probe_timeout(mut self, timeout: Duration) -> Self {
        self.health_db_probe_timeout = timeout;
        self
    }

    fn with_cleanup(mut self, cleanup: CleanupSettings) -> Self {
        self.cleanup = cleanup;
        self
    }

    pub fn mock_default() -> Self {
        let mock = MockProvider::new(MockProviderConfig::default());
        let models = mock.config().supported_models.clone();
        Self::with_provider(Arc::new(mock), models)
    }
}

pub async fn build_state_from_config(
    provider: Arc<dyn ProviderAdapter>,
    models: Vec<String>,
    store: SqliteStore,
    config: &RuntimeConfig,
) -> Result<ProxyState, String> {
    seed_budgets_from_runtime_config(&store, config).await?;
    Ok(ProxyState::with_provider(provider, models)
        .with_store(store)
        .with_detector(Arc::new(DetectEngine::from_runtime_config(&config.detect)))
        .with_mode(mode_from_config(&config.server.mode))
        .with_cleanup(CleanupSettings::from(&config.cleanup))
        .with_session_window_minutes(config.attribution.session_window_minutes as u64))
}

#[derive(Debug, Clone)]
struct RequestContext {
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsRequest {
    model: String,
    #[serde(default)]
    messages: Value,
    #[serde(default)]
    stream: bool,
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

#[derive(Debug, Serialize)]
struct BudgetApiError {
    error: BudgetApiErrorDetail,
}

#[derive(Debug, Serialize)]
struct BudgetApiErrorDetail {
    #[serde(rename = "type")]
    error_type: &'static str,
    retryable: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget: Option<BudgetBlockDetail>,
}

#[derive(Debug, Clone)]
struct NormalizedUsage {
    input_tokens: u64,
    output_tokens: u64,
    source: UsageSource,
    pricing_snapshot: Value,
}

#[derive(Debug, Clone, Copy)]
struct BudgetEnforcementContext {
    estimated_cost_usd: Money,
    reserve_persisted: bool,
}

#[derive(Debug, Clone, Copy)]
struct CleanupSettings {
    strip_ansi: bool,
    minify_json: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct CleanupMetrics {
    input_bytes: u64,
    output_bytes: u64,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        Self {
            strip_ansi: true,
            minify_json: false,
        }
    }
}

impl CleanupSettings {
    fn enabled(self) -> bool {
        self.strip_ansi || self.minify_json
    }
}

impl CleanupMetrics {
    fn from_lengths(input_bytes: usize, output_bytes: usize) -> Self {
        Self {
            input_bytes: input_bytes as u64,
            output_bytes: output_bytes as u64,
        }
    }

    fn bytes_saved(self) -> u64 {
        self.input_bytes.saturating_sub(self.output_bytes)
    }

    fn merge(&mut self, other: CleanupMetrics) {
        self.input_bytes = self.input_bytes.saturating_add(other.input_bytes);
        self.output_bytes = self.output_bytes.saturating_add(other.output_bytes);
    }
}

impl From<&RuntimeCleanupConfig> for CleanupSettings {
    fn from(config: &RuntimeCleanupConfig) -> Self {
        Self {
            strip_ansi: config.strip_ansi,
            minify_json: config.minify_json,
        }
    }
}

pub fn build_router(state: ProxyState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(post_chat_completions))
        .route("/v1/models", get(get_models))
        .route("/internal/health", get(get_internal_health))
        .with_state(Arc::new(state))
        .layer(middleware::from_fn(request_id_middleware))
}

pub async fn serve_default() -> Result<(), ProxyError> {
    serve(DEFAULT_PROXY_BIND).await
}

pub async fn serve(bind: &str) -> Result<(), ProxyError> {
    let addr: SocketAddr = bind.parse()?;
    let listener = TcpListener::bind(addr).await?;
    let app = build_router(ProxyState::mock_default());
    axum::serve(listener, app).await?;
    Ok(())
}

async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = Uuid::now_v7().to_string();
    req.extensions_mut().insert(RequestContext {
        request_id: request_id.clone(),
    });
    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

async fn post_chat_completions(
    State(state): State<Arc<ProxyState>>,
    Extension(ctx): Extension<RequestContext>,
    headers: HeaderMap,
    Json(payload): Json<ChatCompletionsRequest>,
) -> Response {
    if let Err(message) = validate_chat_request(&payload) {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request", message);
    }

    let mut normalized = normalize_chat_request(&ctx, &state, &payload);
    let (project_id, session_id) = match resolve_attribution(&state, &headers).await {
        Ok(attribution) => attribution,
        Err(message) => {
            return internal_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "attribution_failed",
                "failed to resolve request attribution".to_string(),
                message,
            );
        }
    };
    normalized.project_id = project_id;
    normalized.session_id = session_id;

    if state.detector.is_session_paused(&normalized.session_id) {
        let reason = state
            .detector
            .paused_reason(&normalized.session_id)
            .unwrap_or_else(|| SESSION_PAUSED_LOOP_REASON.to_string());
        return budget_error_response(
            "session_paused_loop_detected",
            format!("session `{}` is paused: {reason}", normalized.session_id),
            None,
        );
    }

    let enforcement = match evaluate_budget_before_dispatch(&state, &normalized).await {
        Ok(context) => context,
        Err(response) => return response,
    };

    let provider_response = match state.provider.send(normalized.clone()).await {
        Ok(response) => response,
        Err(ProviderError::UnsupportedModel(model)) => {
            if let Some(context) = enforcement {
                maybe_release_on_dispatch_failure(
                    &state,
                    &normalized.id,
                    context.reserve_persisted,
                )
                .await;
            }
            return error_response(
                StatusCode::BAD_REQUEST,
                "unsupported_model",
                format!("model `{model}` is not configured in the active provider"),
            );
        }
        Err(ProviderError::InternalState(message)) => {
            if let Some(context) = enforcement {
                maybe_release_on_dispatch_failure(
                    &state,
                    &normalized.id,
                    context.reserve_persisted,
                )
                .await;
            }
            return error_response(
                StatusCode::BAD_GATEWAY,
                "provider_internal_state",
                format!("provider internal state error: {message}"),
            );
        }
    };

    let status = StatusCode::from_u16(provider_response.status).unwrap_or(StatusCode::BAD_GATEWAY);
    match provider_response.body {
        ResponseBody::Complete(mut value) => {
            if !status.is_success() {
                if let Some(context) = enforcement.as_ref() {
                    maybe_release_on_dispatch_failure(
                        &state,
                        &normalized.id,
                        context.reserve_persisted,
                    )
                    .await;
                }
                if status.is_server_error() {
                    log_provider_failure_event(
                        &state,
                        &normalized,
                        status,
                        "upstream_http",
                        &value,
                    )
                    .await;
                }
                return (status, Json(value)).into_response();
            }

            let mut usage = provider_usage_from_completion(&value)
                .unwrap_or_else(|| estimated_usage_from_request(&normalized));
            let cleanup_metrics = apply_cleanup_to_json(&mut value, state.cleanup);
            attach_cleanup_metrics(&mut usage, state.cleanup, cleanup_metrics);
            let usage_cost_usd =
                compute_usage_cost_or_fallback(&state, &normalized, &usage, enforcement.as_ref())
                    .await;
            if let Some(context) = enforcement {
                if let Err(message) = reconcile_after_dispatch(
                    &state,
                    &normalized.id,
                    usage_cost_usd,
                    context.reserve_persisted,
                )
                .await
                {
                    return internal_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "ledger_reconcile_failed",
                        "failed to reconcile request accounting".to_string(),
                        message,
                    );
                }
            }
            if let Err(message) =
                persist_request_and_usage(&state, &normalized, &usage, usage_cost_usd).await
            {
                return internal_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "persistence_failed",
                    "failed to persist request metadata".to_string(),
                    message,
                );
            }
            spawn_detection_after_reconcile(
                state.clone(),
                normalized.clone(),
                usage.clone(),
                usage_cost_usd,
            );

            (status, Json(value)).into_response()
        }
        ResponseBody::Stream(_) => {
            if let Some(receiver) = state.provider.take_stream_receiver(&normalized) {
                stream_passthrough_response(
                    status,
                    state.clone(),
                    normalized.clone(),
                    enforcement,
                    receiver,
                )
                .await
            } else if let Some(lines) = state.provider.stream_response_lines(&normalized) {
                let (tx, rx) = mpsc::channel::<String>(lines.len().max(1));
                tokio::spawn(async move {
                    for line in lines {
                        if tx.send(line).await.is_err() {
                            return;
                        }
                    }
                });
                stream_passthrough_response(
                    status,
                    state.clone(),
                    normalized.clone(),
                    enforcement,
                    rx,
                )
                .await
            } else {
                if let Some(context) = enforcement {
                    maybe_release_on_dispatch_failure(
                        &state,
                        &normalized.id,
                        context.reserve_persisted,
                    )
                    .await;
                }
                error_response(
                    StatusCode::NOT_IMPLEMENTED,
                    "streaming_not_supported",
                    "provider reported stream mode but no stream payload is available".to_string(),
                )
            }
        }
    }
}

async fn get_models(State(state): State<Arc<ProxyState>>) -> Json<Value> {
    let data: Vec<Value> = state
        .models
        .iter()
        .map(|model_id| {
            json!({
                "id": model_id,
                "object": "model",
                "owned_by": state.provider.provider_id()
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": data
    }))
}

async fn get_internal_health(State(state): State<Arc<ProxyState>>, headers: HeaderMap) -> Response {
    if header_value(&headers, INTERNAL_HEALTH_HEADER).as_deref()
        != Some(INTERNAL_HEALTH_HEADER_VALUE)
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    let (db_status, db_error, status_code) = match &state.store {
        None => ("disabled", None, StatusCode::OK),
        Some(store) => match timeout(
            state.health_db_probe_timeout,
            query("SELECT 1").fetch_one(store.pool()),
        )
        .await
        {
            Ok(Ok(_)) => ("up", None, StatusCode::OK),
            Ok(Err(err)) => {
                log_internal_error("internal_health_db_probe_failed", err.to_string());
                (
                    "down",
                    Some("unavailable".to_string()),
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            }
            Err(_) => {
                log_internal_error(
                    "internal_health_db_probe_timeout",
                    format!("probe exceeded {:?}", state.health_db_probe_timeout),
                );
                (
                    "down",
                    Some("probe_timeout".to_string()),
                    StatusCode::SERVICE_UNAVAILABLE,
                )
            }
        },
    };

    let uptime_seconds = state.started_at.elapsed().as_secs();
    let body = json!({
        "status": if status_code == StatusCode::OK { "ok" } else { "degraded" },
        "uptime_seconds": uptime_seconds,
        "db": {
            "status": db_status,
            "error": db_error
        },
        "providers": [state.provider.provider_id()],
    });

    (status_code, Json(body)).into_response()
}

fn validate_chat_request(payload: &ChatCompletionsRequest) -> Result<(), String> {
    if extract_model(&payload.model).is_empty() {
        return Err("field `model` must not be empty".to_string());
    }

    let Some(messages) = payload.messages.as_array() else {
        return Err("field `messages` must be an array".to_string());
    };

    if messages.is_empty() {
        return Err("field `messages` must contain at least one message".to_string());
    }

    Ok(())
}

fn normalize_chat_request(
    ctx: &RequestContext,
    state: &ProxyState,
    payload: &ChatCompletionsRequest,
) -> NormalizedRequest {
    let model = extract_model(&payload.model);
    let estimate = estimate_tokens(&payload.messages);

    NormalizedRequest {
        id: ctx.request_id.clone(),
        project_id: state.default_project_id.clone(),
        session_id: state.default_session_id.clone(),
        model_requested: model.clone(),
        model_resolved: model,
        provider_id: state.provider.provider_id().to_string(),
        messages: payload.messages.clone(),
        stream: payload.stream,
        estimated_input_tokens: estimate.input_tokens,
        estimated_output_tokens: estimate.output_tokens,
        timestamp: Utc::now(),
    }
}

fn extract_model(raw: &str) -> String {
    raw.trim().to_string()
}

fn provider_usage_from_completion(payload: &Value) -> Option<NormalizedUsage> {
    let usage = payload.get("usage")?;
    let input_tokens = usage.get("prompt_tokens")?.as_u64()?;
    let output_tokens = usage.get("completion_tokens")?.as_u64()?;
    Some(NormalizedUsage {
        input_tokens,
        output_tokens,
        source: UsageSource::Provider,
        pricing_snapshot: usage.clone(),
    })
}

fn apply_cleanup_to_json(value: &mut Value, cleanup: CleanupSettings) -> CleanupMetrics {
    if !cleanup.enabled() {
        return CleanupMetrics::default();
    }
    match value {
        Value::String(text) => {
            let before = text.len();
            *text = cleanup_text(text, cleanup);
            CleanupMetrics::from_lengths(before, text.len())
        }
        Value::Array(items) => {
            let mut total = CleanupMetrics::default();
            for item in items {
                total.merge(apply_cleanup_to_json(item, cleanup));
            }
            total
        }
        Value::Object(map) => {
            let mut total = CleanupMetrics::default();
            for nested in map.values_mut() {
                total.merge(apply_cleanup_to_json(nested, cleanup));
            }
            total
        }
        _ => CleanupMetrics::default(),
    }
}

fn cleanup_text(input: &str, cleanup: CleanupSettings) -> String {
    let mut output = if cleanup.strip_ansi {
        strip_ansi_sequences(input)
    } else {
        input.to_string()
    };
    if cleanup.minify_json {
        if let Some(minified) = minify_json_string(&output) {
            output = minified;
        }
    }
    output
}

fn strip_ansi_sequences(input: &str) -> String {
    static CSI_RE: OnceLock<Regex> = OnceLock::new();
    static OSC_RE: OnceLock<Regex> = OnceLock::new();
    let csi = CSI_RE
        .get_or_init(|| Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").expect("valid ANSI CSI regex"));
    let osc = OSC_RE.get_or_init(|| {
        Regex::new(r"\x1B\][^\x1B\x07]*(\x07|\x1B\\)").expect("valid ANSI OSC regex")
    });
    let no_csi = csi.replace_all(input, "");
    osc.replace_all(&no_csi, "").into_owned()
}

fn minify_json_string(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    serde_json::to_string(&parsed).ok()
}

fn payload_requires_json_roundtrip(payload: &str, cleanup: CleanupSettings) -> bool {
    if cleanup.minify_json {
        return true;
    }
    if !cleanup.strip_ansi {
        return false;
    }
    payload.contains('\u{1b}') || payload.contains("\\u001b") || payload.contains("\\u001B")
}

fn cleanup_sse_line(line: &str, cleanup: CleanupSettings) -> (String, CleanupMetrics) {
    if !cleanup.enabled() {
        return (line.to_string(), CleanupMetrics::default());
    }

    let trimmed = line.trim_end_matches(['\r', '\n']);
    let trailing_newline = &line[trimmed.len()..];
    let Some(data) = trimmed.strip_prefix("data:") else {
        let cleaned = if cleanup.strip_ansi {
            strip_ansi_sequences(line)
        } else {
            line.to_string()
        };
        let metrics = CleanupMetrics::from_lengths(line.len(), cleaned.len());
        return (cleaned, metrics);
    };
    let payload = data.trim_start();
    if payload.eq_ignore_ascii_case("[DONE]") {
        return (line.to_string(), CleanupMetrics::default());
    }

    if !payload_requires_json_roundtrip(payload, cleanup) {
        return (line.to_string(), CleanupMetrics::default());
    }

    let Ok(mut parsed) = serde_json::from_str::<Value>(payload) else {
        let cleaned = if cleanup.strip_ansi {
            strip_ansi_sequences(line)
        } else {
            line.to_string()
        };
        let metrics = CleanupMetrics::from_lengths(line.len(), cleaned.len());
        return (cleaned, metrics);
    };
    apply_cleanup_to_json(&mut parsed, cleanup);
    match serde_json::to_string(&parsed) {
        Ok(serialized) => {
            let cleaned = format!("data: {serialized}{trailing_newline}");
            let metrics = CleanupMetrics::from_lengths(line.len(), cleaned.len());
            (cleaned, metrics)
        }
        Err(_) => (line.to_string(), CleanupMetrics::default()),
    }
}

fn attach_cleanup_metrics(
    usage: &mut NormalizedUsage,
    cleanup: CleanupSettings,
    metrics: CleanupMetrics,
) {
    if !cleanup.enabled() {
        return;
    }

    let mut snapshot = usage
        .pricing_snapshot
        .as_object()
        .cloned()
        .unwrap_or_default();
    if usage.pricing_snapshot.as_object().is_none() {
        snapshot.insert("raw_snapshot".to_string(), usage.pricing_snapshot.clone());
    }
    snapshot.insert(
        "payload_cleanup".to_string(),
        json!({
            "enabled": true,
            "strip_ansi": cleanup.strip_ansi,
            "minify_json": cleanup.minify_json,
            "bytes_in": metrics.input_bytes,
            "bytes_out": metrics.output_bytes,
            "bytes_saved": metrics.bytes_saved(),
        }),
    );
    usage.pricing_snapshot = Value::Object(snapshot);
}

fn provider_usage_from_sse_lines(lines: &[String]) -> Option<NormalizedUsage> {
    for line in lines {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };

        let data = data.trim();
        if data.eq_ignore_ascii_case("[DONE]") {
            continue;
        }

        let parsed: Value = serde_json::from_str(data).ok()?;
        if let Some(usage) = provider_usage_from_completion(&parsed) {
            return Some(usage);
        }
    }

    None
}

fn estimated_usage_from_request(request: &NormalizedRequest) -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: request.estimated_input_tokens,
        output_tokens: request.estimated_output_tokens,
        source: UsageSource::Estimated,
        pricing_snapshot: json!({
            "source": "token_estimation_hook",
            "model": request.model_resolved
        }),
    }
}

async fn resolve_attribution(
    state: &ProxyState,
    headers: &HeaderMap,
) -> Result<(String, String), String> {
    let project_override =
        header_value(headers, PROJECT_OVERRIDE_HEADER).map(|value| slugify(&value));
    let session_override = header_value(headers, SESSION_OVERRIDE_HEADER);

    let project_path_seed = if let Some(project) = &project_override {
        format!("/override/{project}")
    } else {
        detect_git_root_from(headers)
            .map(|root| root.to_string_lossy().to_string())
            .unwrap_or_else(|| "default".to_string())
    };

    let fallback_project_id = project_override
        .clone()
        .unwrap_or_else(|| project_id_from_seed_path(&project_path_seed));
    let fallback_session_id = session_override
        .clone()
        .unwrap_or_else(|| state.default_session_id.clone());

    let Some(store) = &state.store else {
        return Ok((fallback_project_id, fallback_session_id));
    };

    let project_id = ProjectRepo::upsert_by_path(store, &project_path_seed)
        .await
        .map_err(|err| err.to_string())?;

    let session_id = if let Some(session_id) = session_override {
        ensure_session_override_exists(store, &session_id, &project_id).await?;
        session_id
    } else {
        match SessionRepo::find_active(store, &project_id, state.session_window_minutes)
            .await
            .map_err(|err| err.to_string())?
        {
            Some(session_id) => session_id,
            None => SessionRepo::create(store, &project_id)
                .await
                .map_err(|err| err.to_string())?,
        }
    };

    Ok((project_id, session_id))
}

async fn ensure_session_override_exists(
    store: &SqliteStore,
    session_id: &str,
    project_id: &str,
) -> Result<(), String> {
    query(
        r#"
        INSERT OR IGNORE INTO sessions (id, project_id, source)
        VALUES (?1, ?2, 'header')
        "#,
    )
    .bind(session_id)
    .bind(project_id)
    .execute(store.pool())
    .await
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub async fn seed_budgets_from_runtime_config(
    store: &SqliteStore,
    config: &RuntimeConfig,
) -> Result<Vec<Budget>, String> {
    let mut seeded = Vec::with_capacity(config.budgets.len());
    for (idx, budget) in config.budgets.iter().enumerate() {
        let mapped = map_config_budget(budget, idx)?;
        let stored = BudgetRepo::upsert(store, &mapped)
            .await
            .map_err(|err| err.to_string())?;
        seeded.push(stored);
    }
    Ok(seeded)
}

fn map_config_budget(budget: &RuntimeBudgetConfig, index: usize) -> Result<Budget, String> {
    let hard_limit_usd = budget
        .hard_limit_usd
        .map(Money::from_usd)
        .transpose()
        .map_err(|err| format!("budgets[{index}].hard_limit_usd: {err}"))?;
    let soft_limit_usd = budget
        .soft_limit_usd
        .map(Money::from_usd)
        .transpose()
        .map_err(|err| format!("budgets[{index}].soft_limit_usd: {err}"))?;

    Ok(Budget {
        id: 0,
        scope_type: scope_type_from_config(&budget.scope_type),
        scope_id: budget.scope_id.clone(),
        window_type: window_type_from_config(&budget.window_type),
        hard_limit_usd,
        soft_limit_usd,
        action_on_hard: budget.action_on_hard.clone(),
        action_on_soft: budget.action_on_soft.clone(),
        preset_source: budget.preset_source.clone(),
    })
}

fn scope_type_from_config(scope: &ConfigScopeType) -> ScopeType {
    match scope {
        ConfigScopeType::Global => ScopeType::Global,
        ConfigScopeType::Project => ScopeType::Project,
        ConfigScopeType::Session => ScopeType::Session,
    }
}

fn window_type_from_config(window: &ConfigWindowType) -> WindowType {
    match window {
        ConfigWindowType::Day => WindowType::Day,
        ConfigWindowType::Week => WindowType::Week,
        ConfigWindowType::Month => WindowType::Month,
        ConfigWindowType::Total => WindowType::Total,
    }
}

fn mode_from_config(mode: &penny_config::Mode) -> Mode {
    match mode {
        penny_config::Mode::Observe => Mode::Observe,
        penny_config::Mode::Guard => Mode::Guard,
    }
}

async fn evaluate_budget_before_dispatch(
    state: &ProxyState,
    request: &NormalizedRequest,
) -> Result<Option<BudgetEnforcementContext>, Response> {
    let Some(store) = &state.store else {
        return Ok(None);
    };

    let has_budgets = match has_any_budget(store).await {
        Ok(value) => value,
        Err(message) => {
            if matches!(state.mode, Mode::Guard) {
                log_internal_error("budget_engine_has_any_budget_failed", message);
                return Err(budget_error_response(
                    "budget_engine_failure",
                    "budget engine temporarily unavailable".to_string(),
                    None,
                ));
            }
            false
        }
    };

    if !has_budgets {
        return Ok(Some(BudgetEnforcementContext {
            estimated_cost_usd: Money::ZERO,
            reserve_persisted: false,
        }));
    }

    let estimated_cost_usd = match estimated_request_cost_usd(state, request).await {
        Ok(cost) => cost,
        Err(message) => {
            if matches!(state.mode, Mode::Guard) {
                log_internal_error("budget_engine_estimated_cost_failed", message);
                return Err(budget_error_response(
                    "budget_engine_failure",
                    "budget engine temporarily unavailable".to_string(),
                    None,
                ));
            }
            Money::ZERO
        }
    };

    let evaluator = BudgetEvaluator::new(store.clone(), state.mode.clone());
    match evaluator.evaluate(request, estimated_cost_usd).await {
        RouteDecision::Allow { .. } => {
            let reserve_persisted = has_reserve_entry(store, &request.id).await.unwrap_or(false);
            Ok(Some(BudgetEnforcementContext {
                estimated_cost_usd,
                reserve_persisted,
            }))
        }
        RouteDecision::Block { reason, detail } => {
            let message = format!(
                "{} ({:?}) exceeded: {} / {} - {reason}",
                detail.scope, detail.window, detail.accumulated_usd, detail.limit_usd
            );
            Err(budget_error_response(
                "budget_exceeded",
                message,
                Some(detail),
            ))
        }
        RouteDecision::Failsafe { mode, reason } => {
            if matches!(mode, Mode::Guard) {
                log_internal_error("budget_engine_guard_failsafe", reason);
                Err(budget_error_response(
                    "budget_engine_failure",
                    "budget engine fail-closed in guard mode".to_string(),
                    None,
                ))
            } else {
                Ok(Some(BudgetEnforcementContext {
                    estimated_cost_usd,
                    reserve_persisted: false,
                }))
            }
        }
    }
}

async fn estimated_request_cost_usd(
    state: &ProxyState,
    request: &NormalizedRequest,
) -> Result<Money, String> {
    let Some(store) = &state.store else {
        return Ok(Money::ZERO);
    };
    PricingEngine::new(store)
        .calculate(
            &request.model_resolved,
            request.estimated_input_tokens,
            request.estimated_output_tokens,
        )
        .await
        .map_err(|err| err.to_string())
}

async fn actual_usage_cost_usd(
    state: &ProxyState,
    request: &NormalizedRequest,
    usage: &NormalizedUsage,
) -> Result<Money, String> {
    let Some(store) = &state.store else {
        return Ok(Money::ZERO);
    };
    PricingEngine::new(store)
        .calculate(
            &request.model_resolved,
            usage.input_tokens,
            usage.output_tokens,
        )
        .await
        .map_err(|err| err.to_string())
}

async fn compute_usage_cost_or_fallback(
    state: &ProxyState,
    request: &NormalizedRequest,
    usage: &NormalizedUsage,
    enforcement: Option<&BudgetEnforcementContext>,
) -> Money {
    actual_usage_cost_usd(state, request, usage)
        .await
        .unwrap_or_else(|_| {
            enforcement
                .map(|ctx| ctx.estimated_cost_usd)
                .unwrap_or(Money::ZERO)
        })
}

async fn has_reserve_entry(store: &SqliteStore, request_id: &str) -> Result<bool, String> {
    let reserve_count: i64 = query_scalar(
        "SELECT COUNT(*) FROM cost_ledger WHERE request_id = ?1 AND entry_type = 'reserve'",
    )
    .bind(request_id)
    .fetch_one(store.pool())
    .await
    .map_err(|err| err.to_string())?;
    Ok(reserve_count > 0)
}

async fn has_any_budget(store: &SqliteStore) -> Result<bool, String> {
    let budget_count: i64 = query_scalar("SELECT COUNT(*) FROM budgets")
        .fetch_one(store.pool())
        .await
        .map_err(|err| err.to_string())?;
    Ok(budget_count > 0)
}

async fn reconcile_after_dispatch(
    state: &ProxyState,
    request_id: &str,
    usage_cost_usd: Money,
    reserve_persisted: bool,
) -> Result<(), String> {
    if !reserve_persisted {
        return Ok(());
    }
    let Some(store) = &state.store else {
        return Ok(());
    };
    CostLedger::new(store.clone())
        .reconcile(request_id, usage_cost_usd)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

async fn maybe_release_on_dispatch_failure(
    state: &ProxyState,
    request_id: &str,
    reserve_persisted: bool,
) {
    if !reserve_persisted {
        return;
    }
    let Some(store) = &state.store else {
        return;
    };
    let _ = CostLedger::new(store.clone()).release(request_id).await;
}

async fn persist_request_and_usage(
    state: &ProxyState,
    normalized: &NormalizedRequest,
    usage: &NormalizedUsage,
    cost_usd: Money,
) -> Result<(), String> {
    let Some(store) = &state.store else {
        return Ok(());
    };

    let request = NewRequest {
        id: normalized.id.clone(),
        session_id: Some(normalized.session_id.clone()),
        project_id: normalized.project_id.clone(),
        model_requested: normalized.model_requested.clone(),
        model_used: normalized.model_resolved.clone(),
        provider_id: normalized.provider_id.clone(),
        started_at: normalized.timestamp,
        is_streaming: normalized.stream,
    };
    RequestRepo::insert(store, &request)
        .await
        .map_err(|err| err.to_string())?;

    let usage_record = UsageRecord {
        request_id: normalized.id.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cost_usd,
        source: usage.source.clone(),
        pricing_snapshot: usage.pricing_snapshot.clone(),
    };
    RequestRepo::insert_usage(store, &usage_record)
        .await
        .map_err(|err| err.to_string())?;

    Ok(())
}

async fn stream_passthrough_response(
    status: StatusCode,
    state: Arc<ProxyState>,
    normalized: NormalizedRequest,
    enforcement: Option<BudgetEnforcementContext>,
    mut upstream: mpsc::Receiver<String>,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    tokio::spawn(async move {
        let mut lines = Vec::new();
        let mut cleanup_metrics = CleanupMetrics::default();
        let mut saw_done = false;
        while let Some(line) = upstream.recv().await {
            let (line, line_metrics) = cleanup_sse_line(&line, state.cleanup);
            cleanup_metrics.merge(line_metrics);
            if line.trim().eq_ignore_ascii_case("data: [done]") {
                saw_done = true;
            }
            lines.push(line.clone());
            if client_tx.send(Ok(Bytes::from(line))).await.is_err() {
                break;
            }
            if saw_done {
                break;
            }
        }

        let mut usage = provider_usage_from_sse_lines(&lines)
            .unwrap_or_else(|| estimated_usage_from_request(&normalized));
        attach_cleanup_metrics(&mut usage, state.cleanup, cleanup_metrics);
        if !saw_done {
            mark_stream_incomplete(&mut usage);
            log_provider_failure_event(
                &state,
                &normalized,
                status,
                "incomplete_stream",
                &json!({"message":"upstream stream ended before [DONE]"}),
            )
            .await;
        }

        let usage_cost_usd =
            compute_usage_cost_or_fallback(&state, &normalized, &usage, enforcement.as_ref()).await;
        if let Some(context) = enforcement {
            if let Err(message) = reconcile_after_dispatch(
                &state,
                &normalized.id,
                usage_cost_usd,
                context.reserve_persisted,
            )
            .await
            {
                log_internal_error("ledger_reconcile_failed", message);
            }
        }
        if let Err(message) =
            persist_request_and_usage(&state, &normalized, &usage, usage_cost_usd).await
        {
            log_internal_error("persistence_failed", message);
        }
        spawn_detection_after_reconcile(
            state.clone(),
            normalized.clone(),
            usage.clone(),
            usage_cost_usd,
        );
    });

    let mut response = Response::builder()
        .status(status)
        .body(axum::body::Body::from_stream(ReceiverStream::new(
            client_rx,
        )))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
}

fn mark_stream_incomplete(usage: &mut NormalizedUsage) {
    let mut snapshot = usage
        .pricing_snapshot
        .as_object()
        .cloned()
        .unwrap_or_default();
    snapshot.insert("stream_incomplete".to_string(), Value::Bool(true));
    snapshot.insert("stream_completed".to_string(), Value::Bool(false));
    usage.pricing_snapshot = Value::Object(snapshot);
}

async fn log_provider_failure_event(
    state: &ProxyState,
    normalized: &NormalizedRequest,
    status: StatusCode,
    failure_kind: &'static str,
    payload: &Value,
) {
    let Some(store) = &state.store else {
        return;
    };
    let event = NewEvent {
        request_id: Some(normalized.id.clone()),
        session_id: Some(normalized.session_id.clone()),
        event_type: EventType::ProviderFailure,
        severity: Severity::Error,
        detail: json!({
            "kind": failure_kind,
            "provider_id": normalized.provider_id,
            "model_requested": normalized.model_requested,
            "model_resolved": normalized.model_resolved,
            "http_status": status.as_u16(),
            "error": payload.get("error").cloned().unwrap_or_else(|| payload.clone()),
        }),
    };
    if let Err(err) = EventRepo::insert(store, &event).await {
        log_internal_error("provider_failure_event_insert_failed", err.to_string());
    }
}

async fn run_detection_after_reconcile(
    state: &ProxyState,
    normalized: &NormalizedRequest,
    usage: &NormalizedUsage,
    usage_cost_usd: Money,
) -> Result<(), String> {
    if !state.detector.config().enabled {
        return Ok(());
    }

    let digest = RequestDigest {
        model: normalized.model_resolved.clone(),
        input_tokens: usage.input_tokens,
        cost_usd: usage_cost_usd,
        tool_name: detect_tool_name(&normalized.messages),
        tool_succeeded: true,
        content_hash: detect_content_hash(&normalized.messages),
        timestamp: normalized.timestamp,
    };
    let result = state
        .detector
        .feed(&normalized.session_id, Some(&normalized.id), digest);
    if result.events.is_empty() {
        return Ok(());
    }

    persist_detect_events(state, &result.events).await
}

fn spawn_detection_after_reconcile(
    state: Arc<ProxyState>,
    normalized: NormalizedRequest,
    usage: NormalizedUsage,
    usage_cost_usd: Money,
) {
    tokio::spawn(async move {
        if let Err(message) =
            run_detection_after_reconcile(&state, &normalized, &usage, usage_cost_usd).await
        {
            log_internal_error("detect_integration_failed", message);
        }
    });
}

async fn persist_detect_events(
    state: &ProxyState,
    events: &[DetectEventRecord],
) -> Result<(), String> {
    let Some(store) = &state.store else {
        return Ok(());
    };
    for event in events {
        let new_event = NewEvent {
            request_id: event.request_id.clone(),
            session_id: Some(event.session_id.clone()),
            event_type: event.event_type.clone(),
            severity: event.severity.clone(),
            detail: event.detail.clone(),
        };
        EventRepo::insert(store, &new_event)
            .await
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn detect_tool_name(messages: &Value) -> Option<String> {
    let array = messages.as_array()?;
    for message in array {
        if let Some(tool) = message.get("tool_name").and_then(Value::as_str) {
            let tool = tool.trim();
            if !tool.is_empty() {
                return Some(tool.to_string());
            }
        }
    }
    None
}

fn detect_content_hash(messages: &Value) -> u64 {
    let content = first_user_message_text(messages).unwrap_or_default();
    let mut hasher = Fnv1a64::default();
    let mut chars = content.chars();
    for ch in chars.by_ref().take(500) {
        let mut bytes = [0_u8; 4];
        let encoded = ch.encode_utf8(&mut bytes);
        hasher.write(encoded.as_bytes());
    }
    hasher.finish()
}

fn first_user_message_text(messages: &Value) -> Option<String> {
    let array = messages.as_array()?;
    for message in array {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if role != "user" {
            continue;
        }
        let content = message.get("content")?;
        return extract_message_text(content);
    }
    None
}

fn extract_message_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
        return None;
    }

    let blocks = content.as_array()?;
    let joined = blocks
        .iter()
        .filter_map(|entry| entry.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let joined = joined.trim();
    if joined.is_empty() {
        None
    } else {
        Some(joined.to_string())
    }
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        const OFFSET: u64 = 0xcbf29ce484222325;
        const PRIME: u64 = 0x100000001b3;
        if self.0 == 0 {
            self.0 = OFFSET;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }
}

fn detect_git_root_from(headers: &HeaderMap) -> Option<PathBuf> {
    let start_path = header_value(headers, CWD_OVERRIDE_HEADER)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    detect_git_root(&start_path)
}

fn detect_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn project_id_from_seed_path(path: &str) -> String {
    if path == "default" {
        return "default".to_string();
    }

    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(slugify)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn slugify(input: &str) -> String {
    let source = input.trim().to_lowercase();
    let mut output = String::with_capacity(source.len());
    let mut prev_dash = false;
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            output.push('-');
            prev_dash = true;
        }
    }
    output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "default".to_string()
    } else {
        output
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn error_response(status: StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(json!(ApiError {
            error: ApiErrorDetail { code, message }
        })),
    )
        .into_response()
}

fn internal_error_response(
    status: StatusCode,
    code: &'static str,
    public_message: String,
    internal_detail: String,
) -> Response {
    log_internal_error(code, internal_detail);
    error_response(status, code, public_message)
}

fn budget_error_response(
    error_type: &'static str,
    message: String,
    budget: Option<BudgetBlockDetail>,
) -> Response {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(json!(BudgetApiError {
            error: BudgetApiErrorDetail {
                error_type,
                retryable: false,
                message,
                budget,
            }
        })),
    )
        .into_response()
}

fn log_internal_error(tag: &str, detail: String) {
    error!(tag = tag, detail = %detail, "internal proxy error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use penny_budget::BudgetEvaluator;
    use penny_config::{load_config, LoadOptions, LoopAction};
    use penny_cost::import_pricebook_files;
    use penny_detect::{DetectEngine, DetectorConfig, SESSION_PAUSED_LOOP_REASON};
    use penny_store::BudgetRepo;
    use penny_types::{
        Budget, Money, ProviderResponse, RequestDigest, RouteDecision, ScopeType, StreamDescriptor,
        WindowType,
    };
    use sqlx::Row;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio::time::{sleep, Duration};
    use tower::ServiceExt;

    fn app() -> Router {
        build_router(ProxyState::mock_default())
    }

    async fn app_with_store() -> (Router, SqliteStore) {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let state = ProxyState::mock_default().with_store(store.clone());
        (build_router(state), store)
    }

    async fn app_with_store_and_detector(detector: Arc<DetectEngine>) -> (Router, SqliteStore) {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let state = ProxyState::mock_default()
            .with_store(store.clone())
            .with_detector(detector);
        (build_router(state), store)
    }

    fn detector(loop_action: LoopAction, threshold: u32) -> Arc<DetectEngine> {
        Arc::new(DetectEngine::new(DetectorConfig {
            enabled: true,
            burn_rate_alert_usd_per_hour: 10_000.0,
            loop_window_seconds: 120,
            loop_threshold_similar_requests: threshold,
            loop_action,
            min_burn_rate_observation_seconds: 30,
            max_recorded_events: 5000,
            session_state_retention_seconds: 3600,
            max_sessions: 2048,
        }))
    }

    #[derive(Debug, Default)]
    struct IncompleteStreamProvider {
        pending: Mutex<HashMap<String, mpsc::Receiver<String>>>,
    }

    #[async_trait]
    impl ProviderAdapter for IncompleteStreamProvider {
        async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
            if !req.stream {
                return Ok(ProviderResponse {
                    status: 200,
                    body: ResponseBody::Complete(json!({
                        "id": format!("chatcmpl_{}", req.id),
                        "object": "chat.completion",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "non-stream"},
                            "finish_reason": "stop"
                        }]
                    })),
                    upstream_ms: 0,
                });
            }

            let (tx, rx) = mpsc::channel(8);
            let req_id = req.id.clone();
            tokio::spawn(async move {
                let lines = vec![
                    "data: {\"id\":\"chatcmpl_incomplete\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n".to_string(),
                    "data: {\"id\":\"chatcmpl_incomplete\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n".to_string(),
                ];
                for line in lines {
                    if tx.send(line).await.is_err() {
                        return;
                    }
                }
            });
            self.pending
                .lock()
                .expect("pending lock")
                .insert(req_id, rx);

            Ok(ProviderResponse {
                status: 200,
                body: ResponseBody::Stream(StreamDescriptor {
                    provider: "incomplete".to_string(),
                    format: "sse".to_string(),
                }),
                upstream_ms: 1,
            })
        }

        fn provider_id(&self) -> &str {
            "incomplete"
        }

        fn supports_model(&self, _model: &str) -> bool {
            true
        }

        fn take_stream_receiver(&self, req: &NormalizedRequest) -> Option<mpsc::Receiver<String>> {
            self.pending.lock().ok()?.remove(&req.id)
        }
    }

    #[derive(Debug, Clone)]
    struct StaticErrorProvider {
        status: u16,
        payload: Value,
    }

    #[async_trait]
    impl ProviderAdapter for StaticErrorProvider {
        async fn send(&self, _req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
            Ok(ProviderResponse {
                status: self.status,
                body: ResponseBody::Complete(self.payload.clone()),
                upstream_ms: 0,
            })
        }

        fn provider_id(&self) -> &str {
            "static-error"
        }

        fn supports_model(&self, _model: &str) -> bool {
            true
        }
    }

    async fn app_with_incomplete_stream_provider() -> (Router, SqliteStore) {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let provider: Arc<dyn ProviderAdapter> = Arc::new(IncompleteStreamProvider::default());
        let state = ProxyState::with_provider(provider, vec!["gpt-4.1".to_string()])
            .with_store(store.clone());
        (build_router(state), store)
    }

    async fn app_with_static_error_provider(status: u16, payload: Value) -> (Router, SqliteStore) {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let provider: Arc<dyn ProviderAdapter> = Arc::new(StaticErrorProvider { status, payload });
        let state = ProxyState::with_provider(provider, vec!["claude-sonnet-4-6".to_string()])
            .with_store(store.clone());
        (build_router(state), store)
    }

    #[derive(Debug, Default)]
    struct AnsiContentProvider;

    #[async_trait]
    impl ProviderAdapter for AnsiContentProvider {
        async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
            if req.stream {
                return Ok(ProviderResponse {
                    status: 200,
                    body: ResponseBody::Stream(StreamDescriptor {
                        provider: "ansi".to_string(),
                        format: "sse".to_string(),
                    }),
                    upstream_ms: 0,
                });
            }

            Ok(ProviderResponse {
                status: 200,
                body: ResponseBody::Complete(json!({
                    "id": format!("chatcmpl_{}", req.id),
                    "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "\u{1b}[31mcolored response\u{1b}[0m"},
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 120,
                        "completion_tokens": 48
                    }
                })),
                upstream_ms: 0,
            })
        }

        fn provider_id(&self) -> &str {
            "ansi"
        }

        fn supports_model(&self, _model: &str) -> bool {
            true
        }

        fn stream_response_lines(&self, _req: &NormalizedRequest) -> Option<Vec<String>> {
            let first_chunk = json!({
                "id": "chatcmpl_ansi_stream",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant"},
                    "finish_reason": null
                }]
            });
            let second_chunk = json!({
                "id": "chatcmpl_ansi_stream",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "\u{1b}[31mstream colored\u{1b}[0m"},
                    "finish_reason": null
                }]
            });
            let final_chunk = json!({
                "id": "chatcmpl_ansi_stream",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 120,
                    "completion_tokens": 48
                }
            });
            Some(vec![
                format!("data: {first_chunk}\n\n"),
                format!("data: {second_chunk}\n\n"),
                format!("data: {final_chunk}\n\n"),
                "data: [DONE]\n\n".to_string(),
            ])
        }
    }

    fn app_with_ansi_provider(cleanup: CleanupSettings) -> Router {
        let provider: Arc<dyn ProviderAdapter> = Arc::new(AnsiContentProvider);
        let state = ProxyState::with_provider(provider, vec!["claude-sonnet-4-6".to_string()])
            .with_cleanup(cleanup);
        build_router(state)
    }

    async fn app_with_ansi_provider_and_store(cleanup: CleanupSettings) -> (Router, SqliteStore) {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let provider: Arc<dyn ProviderAdapter> = Arc::new(AnsiContentProvider);
        let state = ProxyState::with_provider(provider, vec!["claude-sonnet-4-6".to_string()])
            .with_store(store.clone())
            .with_cleanup(cleanup);
        (build_router(state), store)
    }

    async fn seed_pricebooks(store: &SqliteStore) {
        let prices_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../prices")
            .canonicalize()
            .expect("resolve prices dir");
        import_pricebook_files(
            store,
            &[
                prices_dir.join("anthropic.toml"),
                prices_dir.join("openai.toml"),
            ],
        )
        .await
        .expect("import pricebooks");
    }

    async fn seed_global_day_budget(store: &SqliteStore, hard_limit_usd: Money) {
        BudgetRepo::upsert(
            store,
            &Budget {
                id: 0,
                scope_type: ScopeType::Global,
                scope_id: "*".to_string(),
                window_type: WindowType::Day,
                hard_limit_usd: Some(hard_limit_usd),
                soft_limit_usd: None,
                action_on_hard: "block".to_string(),
                action_on_soft: "warn".to_string(),
                preset_source: None,
            },
        )
        .await
        .expect("seed budget");
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("resolve repo root")
    }

    fn golden_json(name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("golden")
            .join(name);
        let content = fs::read_to_string(path).expect("golden file");
        serde_json::from_str(&content).expect("golden json")
    }

    fn budget_request(id: &str) -> NormalizedRequest {
        NormalizedRequest {
            id: id.to_string(),
            project_id: "project-alpha".to_string(),
            session_id: "session-1".to_string(),
            model_requested: "claude-sonnet-4-6".to_string(),
            model_resolved: "claude-sonnet-4-6".to_string(),
            provider_id: "mock".to_string(),
            messages: json!([{ "role": "user", "content": "hello" }]),
            stream: false,
            estimated_input_tokens: 100,
            estimated_output_tokens: 50,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn budget_seeding_from_preset_is_idempotent_and_tagged() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let config = load_config(LoadOptions {
            repository_root: Some(repo_root()),
            preset: Some("team".to_string()),
            ..LoadOptions::default()
        })
        .expect("load config with team preset");

        let first = seed_budgets_from_runtime_config(&store, &config)
            .await
            .expect("first seed");
        let second = seed_budgets_from_runtime_config(&store, &config)
            .await
            .expect("second seed");
        assert_eq!(first.len(), config.budgets.len());
        assert_eq!(second.len(), config.budgets.len());

        let all = BudgetRepo::list_all(&store)
            .await
            .expect("list all budgets");
        assert_eq!(all.len(), config.budgets.len());
        assert!(all
            .iter()
            .all(|budget| budget.preset_source.as_deref() == Some("preset:team")));
    }

    #[tokio::test]
    async fn user_budget_override_replaces_preset_values_safely() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let temp = TempDir::new().expect("temp dir");
        let user_config_path = temp.path().join("config.toml");
        fs::write(
            &user_config_path,
            r#"
[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "day"
hard_limit_usd = 7.0
action_on_hard = "block"
action_on_soft = "warn"
"#,
        )
        .expect("write user config");

        let config = load_config(LoadOptions {
            repository_root: Some(repo_root()),
            config_path: Some(user_config_path),
            preset: Some("team".to_string()),
        })
        .expect("load config with user override");

        seed_budgets_from_runtime_config(&store, &config)
            .await
            .expect("seed budgets");

        let all = BudgetRepo::list_all(&store)
            .await
            .expect("list all budgets");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].window_type, WindowType::Day);
        assert_eq!(
            all[0].hard_limit_usd,
            Some(Money::from_usd(7.0).expect("money"))
        );
        assert_eq!(all[0].preset_source, None);
    }

    #[tokio::test]
    async fn seeded_budgets_are_immediately_visible_to_evaluator() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let config = load_config(LoadOptions {
            repository_root: Some(repo_root()),
            preset: Some("indie".to_string()),
            ..LoadOptions::default()
        })
        .expect("load config with indie preset");

        seed_budgets_from_runtime_config(&store, &config)
            .await
            .expect("seed budgets");

        let evaluator = BudgetEvaluator::new(store, Mode::Guard);
        let decision = evaluator
            .evaluate(
                &budget_request("req_seeded_eval"),
                Money::from_usd(11.0).expect("money"),
            )
            .await;
        assert!(matches!(decision, RouteDecision::Block { .. }));
    }

    #[tokio::test]
    async fn post_chat_completions_passes_through_mock_provider() {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"hello"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(REQUEST_ID_HEADER).is_some());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            json_body["choices"][0]["message"]["content"],
            "Mock provider deterministic response."
        );
    }

    #[tokio::test]
    async fn post_chat_completions_strips_ansi_by_default() {
        let app = app_with_ansi_provider(CleanupSettings::default());
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"hello"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            json_body["choices"][0]["message"]["content"],
            "colored response"
        );
    }

    #[tokio::test]
    async fn post_chat_completions_can_disable_payload_cleanup() {
        let app = app_with_ansi_provider(CleanupSettings {
            strip_ansi: false,
            minify_json: false,
        });
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"hello"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            json_body["choices"][0]["message"]["content"],
            "\u{1b}[31mcolored response\u{1b}[0m"
        );
    }

    #[tokio::test]
    async fn stream_passthrough_cleans_ansi_payload_chunks() {
        let app = app_with_ansi_provider(CleanupSettings::default());
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"stream please"}],
                    "stream": true
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let text = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(text.contains("stream colored"));
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("data: [DONE]"));
    }

    #[tokio::test]
    async fn cleanup_metrics_are_persisted_when_enabled() {
        let (app, store) = app_with_ansi_provider_and_store(CleanupSettings::default()).await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"metrics please"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");

        let pricing_snapshot: String =
            sqlx::query_scalar("SELECT pricing_snapshot FROM request_usage WHERE request_id = ?1")
                .bind(&request_id)
                .fetch_one(store.pool())
                .await
                .expect("usage pricing snapshot");
        let pricing_json: Value = serde_json::from_str(&pricing_snapshot).expect("pricing json");
        let metrics = &pricing_json["payload_cleanup"];
        assert_eq!(metrics["enabled"], true);
        assert_eq!(metrics["strip_ansi"], true);
        assert_eq!(metrics["minify_json"], false);
        assert!(
            metrics["bytes_saved"].as_u64().expect("bytes_saved as u64") > 0,
            "expected ANSI cleanup to save bytes: {pricing_json}"
        );
    }

    #[tokio::test]
    async fn stream_cleanup_metrics_are_accumulated_in_usage_snapshot() {
        let (app, store) = app_with_ansi_provider_and_store(CleanupSettings::default()).await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"stream metrics"}],
                    "stream": true
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");

        let _body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read stream body");
        sleep(Duration::from_millis(50)).await;

        let pricing_snapshot: String =
            sqlx::query_scalar("SELECT pricing_snapshot FROM request_usage WHERE request_id = ?1")
                .bind(&request_id)
                .fetch_one(store.pool())
                .await
                .expect("usage pricing snapshot");
        let pricing_json: Value = serde_json::from_str(&pricing_snapshot).expect("pricing json");
        let metrics = &pricing_json["payload_cleanup"];
        let bytes_in = metrics["bytes_in"].as_u64().expect("bytes_in as u64");
        let bytes_out = metrics["bytes_out"].as_u64().expect("bytes_out as u64");
        assert!(bytes_in > 0, "expected non-zero bytes_in: {pricing_json}");
        assert!(
            bytes_in > bytes_out,
            "expected cleanup savings: {pricing_json}"
        );
    }

    #[test]
    fn cleanup_text_minifies_json_when_enabled() {
        let cleanup = CleanupSettings {
            strip_ansi: false,
            minify_json: true,
        };
        let input = " { \"a\": 1, \"b\": [1, 2] } ";
        assert_eq!(cleanup_text(input, cleanup), "{\"a\":1,\"b\":[1,2]}");
    }

    #[test]
    fn cleanup_sse_line_skips_json_roundtrip_when_unneeded() {
        let cleanup = CleanupSettings {
            strip_ansi: true,
            minify_json: false,
        };
        let line = "data: {\"id\":\"x\",\"choices\":[{\"delta\":{\"content\":\"plain\"}}]}\n\n";
        let (cleaned, metrics) = cleanup_sse_line(line, cleanup);
        assert_eq!(cleaned, line);
        assert_eq!(metrics.bytes_saved(), 0);
    }

    #[test]
    fn cleanup_sse_line_strips_escaped_ansi_without_minify() {
        let cleanup = CleanupSettings {
            strip_ansi: true,
            minify_json: false,
        };
        let line = "data: {\"id\":\"x\",\"choices\":[{\"delta\":{\"content\":\"\\u001b[31mhello\\u001b[0m\"}}]}\n\n";
        let (cleaned, metrics) = cleanup_sse_line(line, cleanup);
        assert!(cleaned.contains("hello"));
        assert!(!cleaned.contains("\\u001b"));
        assert!(metrics.bytes_saved() > 0);
    }

    #[tokio::test]
    async fn post_chat_completions_invalid_payload_fails_clearly() {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "",
                    "messages": []
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get(REQUEST_ID_HEADER).is_some());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body, golden_json("invalid_request_error.json"));
    }

    #[tokio::test]
    async fn get_models_returns_openai_compatible_model_list() {
        let request = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .body(Body::empty())
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(REQUEST_ID_HEADER).is_some());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["object"], "list");
        assert!(json_body["data"]
            .as_array()
            .expect("array data")
            .iter()
            .any(|item| item["id"] == "claude-sonnet-4-6"));
    }

    #[tokio::test]
    async fn get_internal_health_returns_status_and_provider_data() {
        let request = Request::builder()
            .method("GET")
            .uri("/internal/health")
            .header(INTERNAL_HEALTH_HEADER, INTERNAL_HEALTH_HEADER_VALUE)
            .body(Body::empty())
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(REQUEST_ID_HEADER).is_some());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["status"], "ok");
        assert_eq!(json_body["db"]["status"], "disabled");
        assert!(json_body["uptime_seconds"].as_u64().is_some());
        assert!(json_body["providers"]
            .as_array()
            .expect("providers array")
            .iter()
            .any(|item| item == "mock"));
    }

    #[tokio::test]
    async fn get_internal_health_reports_db_up_when_store_is_available() {
        let (app, _store) = app_with_store().await;
        let request = Request::builder()
            .method("GET")
            .uri("/internal/health")
            .header(INTERNAL_HEALTH_HEADER, INTERNAL_HEALTH_HEADER_VALUE)
            .body(Body::empty())
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["db"]["status"], "up");
    }

    #[tokio::test]
    async fn internal_health_requires_internal_header() {
        let request = Request::builder()
            .method("GET")
            .uri("/internal/health")
            .body(Body::empty())
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn internal_health_db_probe_timeout_returns_degraded_fast() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        let state = ProxyState::mock_default()
            .with_store(store.clone())
            .with_health_db_probe_timeout(Duration::from_millis(5));
        let app = build_router(state);

        let _held_conn = store.pool().acquire().await.expect("hold connection");
        let request = Request::builder()
            .method("GET")
            .uri("/internal/health")
            .header(INTERNAL_HEALTH_HEADER, INTERNAL_HEALTH_HEADER_VALUE)
            .body(Body::empty())
            .expect("build request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["status"], "degraded");
        assert_eq!(json_body["db"]["status"], "down");
        assert_eq!(json_body["db"]["error"], "probe_timeout");
    }

    #[tokio::test]
    async fn attribution_error_response_is_sanitized() {
        let (app, store) = app_with_store().await;
        sqlx::query("DROP TABLE projects")
            .execute(store.pool())
            .await
            .expect("drop projects table");

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"trigger attribution failure"}]
                })
                .to_string(),
            ))
            .expect("build request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body, golden_json("attribution_failed_error.json"));
        let msg = json_body["error"]["message"]
            .as_str()
            .expect("error message as str");
        assert!(!msg.contains("sqlx"));
        assert!(!msg.contains("no such table"));
    }

    #[tokio::test]
    async fn persistence_error_response_is_sanitized() {
        let (app, store) = app_with_store().await;
        sqlx::query("DROP TABLE request_usage")
            .execute(store.pool())
            .await
            .expect("drop request_usage table");

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"trigger persistence failure"}]
                })
                .to_string(),
            ))
            .expect("build request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body, golden_json("persistence_failed_error.json"));
        let msg = json_body["error"]["message"]
            .as_str()
            .expect("error message as str");
        assert!(!msg.contains("sqlx"));
        assert!(!msg.contains("no such table"));
    }

    #[tokio::test]
    async fn streaming_requests_return_sse_payload() {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"stream please"}],
                    "stream": true
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream; charset=utf-8")
        );
        assert!(response.headers().get(REQUEST_ID_HEADER).is_some());

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let text = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(text.contains("data: [DONE]"));
        assert!(text.contains("\"usage\""));
    }

    #[tokio::test]
    async fn interrupted_streams_are_marked_incomplete_and_accounted() {
        let (app, store) = app_with_incomplete_stream_provider().await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4.1",
                    "messages": [{"role":"user","content":"interrupt me"}],
                    "stream": true
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let text = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(text.contains("\"partial\""));
        assert!(!text.contains("data: [DONE]"));

        sleep(Duration::from_millis(50)).await;
        let row = sqlx::query(
            "SELECT source, pricing_snapshot, input_tokens, output_tokens FROM request_usage WHERE request_id = ?1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("usage row");

        assert_eq!(row.get::<String, _>("source"), "estimated");
        let pricing_snapshot: String = row.get("pricing_snapshot");
        let pricing_json: Value =
            serde_json::from_str(&pricing_snapshot).expect("pricing snapshot json");
        assert_eq!(pricing_json["stream_incomplete"], true);
        assert_eq!(pricing_json["stream_completed"], false);
        assert!(row.get::<i64, _>("input_tokens") > 0);
        assert!(row.get::<i64, _>("output_tokens") > 0);

        let detail: String = sqlx::query_scalar(
            "SELECT detail FROM events WHERE request_id = ?1 AND event_type = 'provider_failure' ORDER BY id DESC LIMIT 1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("provider failure event");
        let detail_json: Value = serde_json::from_str(&detail).expect("event detail json");
        assert_eq!(detail_json["kind"], "incomplete_stream");
        assert_eq!(detail_json["http_status"], 200);
    }

    #[test]
    fn normalization_extracts_model_stream_and_token_hook_fields() {
        let state = ProxyState::mock_default();
        let ctx = RequestContext {
            request_id: "req_norm_01".to_string(),
        };
        let payload = ChatCompletionsRequest {
            model: "  claude-sonnet-4-6 ".to_string(),
            messages: json!([{ "role": "user", "content": "normalization test" }]),
            stream: true,
        };

        let normalized = normalize_chat_request(&ctx, &state, &payload);
        assert_eq!(normalized.id, "req_norm_01");
        assert_eq!(normalized.model_requested, "claude-sonnet-4-6");
        assert_eq!(normalized.model_resolved, "claude-sonnet-4-6");
        assert!(normalized.stream);
        assert!(normalized.estimated_input_tokens > 0);
        assert!(normalized.estimated_output_tokens > 0);
    }

    #[tokio::test]
    async fn request_and_usage_are_persisted_with_traceable_id_and_timestamp() {
        let (app, store) = app_with_store().await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": " claude-sonnet-4-6 ",
                    "messages": [{"role":"user","content":"persist me"}],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id header")
            .to_string();

        let row = sqlx::query(
            "SELECT id, model_requested, model_used, is_streaming, started_at, session_id, project_id FROM requests WHERE id = ?1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("request row");
        assert_eq!(row.get::<String, _>("id"), request_id);
        assert_eq!(row.get::<String, _>("model_requested"), "claude-sonnet-4-6");
        assert_eq!(row.get::<String, _>("model_used"), "claude-sonnet-4-6");
        assert_eq!(row.get::<i64, _>("is_streaming"), 0);
        assert!(!row.get::<String, _>("session_id").is_empty());
        assert!(!row.get::<String, _>("project_id").is_empty());

        let started_at = row.get::<String, _>("started_at");
        assert!(
            chrono::DateTime::parse_from_rfc3339(&started_at).is_ok(),
            "started_at should be RFC3339, got {started_at}"
        );

        let usage = sqlx::query(
            "SELECT request_id, input_tokens, output_tokens FROM request_usage WHERE request_id = ?1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("usage row");
        assert_eq!(usage.get::<String, _>("request_id"), request_id);
        assert_eq!(usage.get::<i64, _>("input_tokens"), 120);
        assert_eq!(usage.get::<i64, _>("output_tokens"), 48);
    }

    #[tokio::test]
    async fn over_budget_request_returns_structured_402_and_never_429() {
        let (app, store) = app_with_store().await;
        seed_pricebooks(&store).await;
        seed_global_day_budget(&store, Money::from_usd(0.0).expect("money")).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"should block"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json_body["error"]["type"], "budget_exceeded");
        assert_eq!(json_body["error"]["retryable"], false);
        assert_eq!(json_body["error"]["budget"]["scope"], "global:*");
        assert_eq!(json_body["error"]["budget"]["window"], "day");
        assert!(
            json_body["error"]["budget"]["accumulated_usd"]
                .as_f64()
                .expect("accumulated as f64")
                > 0.0
        );
        assert_eq!(json_body["error"]["budget"]["limit_usd"], 0.0);
        assert!(json_body["error"]["budget"]["resets_at"].is_string());
    }

    #[tokio::test]
    async fn paused_session_short_circuits_before_budget_evaluation() {
        let detector = detector(LoopAction::Pause, 1);
        detector.feed(
            "sess-paused",
            Some("seed-req"),
            RequestDigest {
                model: "claude-sonnet-4-6".to_string(),
                input_tokens: 100,
                cost_usd: Money::from_usd(0.1).expect("money"),
                tool_name: None,
                tool_succeeded: true,
                content_hash: 99,
                timestamp: Utc::now(),
            },
        );
        assert!(detector.is_session_paused("sess-paused"));

        let (app, store) = app_with_store_and_detector(detector).await;
        seed_pricebooks(&store).await;
        seed_global_day_budget(&store, Money::from_usd(10.0).expect("money")).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(SESSION_OVERRIDE_HEADER, "sess-paused")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"should be blocked by detect pause"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["error"]["type"], "session_paused_loop_detected");
        let message = json_body["error"]["message"]
            .as_str()
            .expect("message as str");
        assert!(message.contains(SESSION_PAUSED_LOOP_REASON));

        let ledger_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cost_ledger WHERE request_id = ?1")
                .bind(&request_id)
                .fetch_one(store.pool())
                .await
                .expect("ledger count");
        assert_eq!(ledger_rows, 0);

        let request_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests WHERE id = ?1")
            .bind(&request_id)
            .fetch_one(store.pool())
            .await
            .expect("request count");
        assert_eq!(request_rows, 0);
    }

    #[tokio::test]
    async fn detect_events_are_persisted_after_reconcile() {
        let detector = detector(LoopAction::Alert, 1);
        let (app, store) = app_with_store_and_detector(detector).await;
        seed_pricebooks(&store).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(PROJECT_OVERRIDE_HEADER, "proj-detect")
            .header(SESSION_OVERRIDE_HEADER, "sess-detect")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"trigger detect event"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");

        let mut detail = None;
        for _ in 0..20 {
            detail = sqlx::query_scalar(
                "SELECT detail FROM events WHERE request_id = ?1 AND event_type = 'loop_detected' ORDER BY id DESC LIMIT 1",
            )
            .bind(&request_id)
            .fetch_optional(store.pool())
            .await
            .expect("detect event detail query");
            if detail.is_some() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        let detail: String = detail.expect("detect event detail");
        let detail_json: Value = serde_json::from_str(&detail).expect("detail json");
        assert_eq!(detail_json["kind"], "content_similarity");
        assert!(
            detail_json["similar_count"]
                .as_u64()
                .expect("similar_count as u64")
                >= 1
        );
    }

    #[tokio::test]
    async fn provider_429_passthrough_is_distinct_from_budget_failure() {
        let (app, _store) = app_with_static_error_provider(
            429,
            json!({
                "error": {
                    "message": "provider rate limit",
                    "type": "rate_limit_error",
                    "code": 429
                }
            }),
        )
        .await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"rate-limit check"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_ne!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["error"]["type"], "rate_limit_error");
        assert!(json_body["error"]["budget"].is_null());
        assert!(json_body["error"]["retryable"].is_null());
    }

    #[tokio::test]
    async fn provider_5xx_passthrough_logs_provider_failure_event() {
        let (app, store) = app_with_static_error_provider(
            503,
            json!({
                "error": {
                    "message": "upstream overloaded",
                    "type": "server_error",
                    "code": 503
                }
            }),
        )
        .await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"5xx check"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
            .expect("request id header");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let json_body: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json_body["error"]["type"], "server_error");

        let detail: String = sqlx::query_scalar(
            "SELECT detail FROM events WHERE request_id = ?1 AND event_type = 'provider_failure' ORDER BY id DESC LIMIT 1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("provider failure event");
        let detail_json: Value = serde_json::from_str(&detail).expect("event detail json");
        assert_eq!(detail_json["kind"], "upstream_http");
        assert_eq!(detail_json["http_status"], 503);
        assert_eq!(detail_json["provider_id"], "static-error");
    }

    #[tokio::test]
    async fn successful_dispatch_persists_reserve_then_reconcile() {
        let (app, store) = app_with_store().await;
        seed_pricebooks(&store).await;
        seed_global_day_budget(&store, Money::from_usd(10.0).expect("money")).await;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"reserve and reconcile"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id header");

        let entry_types: Vec<String> = sqlx::query_scalar(
            "SELECT entry_type FROM cost_ledger WHERE request_id = ?1 ORDER BY id",
        )
        .bind(request_id)
        .fetch_all(store.pool())
        .await
        .expect("ledger entries");
        assert_eq!(entry_types, vec!["reserve", "reconcile"]);
    }

    #[tokio::test]
    async fn dispatch_failure_releases_reserved_amount() {
        let store = SqliteStore::connect("sqlite::memory:")
            .await
            .expect("create in-memory store");
        seed_pricebooks(&store).await;
        seed_global_day_budget(&store, Money::from_usd(10.0).expect("money")).await;

        let failing_provider = MockProvider::new(MockProviderConfig {
            supported_models: Vec::new(),
            ..MockProviderConfig::default()
        });
        let state = ProxyState::with_provider(Arc::new(failing_provider), Vec::new())
            .with_store(store.clone())
            .with_mode(Mode::Guard);
        let app = build_router(state);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"force dispatch failure"}]
                })
                .to_string(),
            ))
            .expect("build request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id header");
        let entry_types: Vec<String> = sqlx::query_scalar(
            "SELECT entry_type FROM cost_ledger WHERE request_id = ?1 ORDER BY id",
        )
        .bind(request_id)
        .fetch_all(store.pool())
        .await
        .expect("ledger entries");
        assert_eq!(entry_types, vec!["reserve", "release"]);
    }

    #[tokio::test]
    async fn same_project_within_window_reuses_session() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("create git marker");
        let cwd = temp.path().to_string_lossy().to_string();

        let (app, store) = app_with_store().await;
        let request_one = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(CWD_OVERRIDE_HEADER, cwd.clone())
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"one"}]
                })
                .to_string(),
            ))
            .expect("request one");
        let request_two = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(CWD_OVERRIDE_HEADER, cwd)
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"two"}]
                })
                .to_string(),
            ))
            .expect("request two");

        let response_one = app.clone().oneshot(request_one).await.expect("resp one");
        let response_two = app.oneshot(request_two).await.expect("resp two");
        let request_id_one = response_one
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .expect("request id one");
        let request_id_two = response_two
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .expect("request id two");

        let session_one: String =
            sqlx::query_scalar("SELECT session_id FROM requests WHERE id = ?1")
                .bind(request_id_one)
                .fetch_one(store.pool())
                .await
                .expect("session one");
        let session_two: String =
            sqlx::query_scalar("SELECT session_id FROM requests WHERE id = ?1")
                .bind(request_id_two)
                .fetch_one(store.pool())
                .await
                .expect("session two");

        assert_eq!(session_one, session_two);
    }

    #[tokio::test]
    async fn missing_git_root_falls_back_to_default_project() {
        let temp = TempDir::new().expect("temp dir");
        let cwd = temp.path().to_string_lossy().to_string();

        let (app, store) = app_with_store().await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(CWD_OVERRIDE_HEADER, cwd)
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"fallback"}]
                })
                .to_string(),
            ))
            .expect("request");

        let response = app.oneshot(request).await.expect("response");
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .expect("request id");
        let project_id: String =
            sqlx::query_scalar("SELECT project_id FROM requests WHERE id = ?1")
                .bind(request_id)
                .fetch_one(store.pool())
                .await
                .expect("project id");

        assert_eq!(project_id, "default");
    }

    #[tokio::test]
    async fn explicit_project_and_session_headers_override_defaults() {
        let (app, store) = app_with_store().await;
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .header(PROJECT_OVERRIDE_HEADER, "Custom Project Name")
            .header(SESSION_OVERRIDE_HEADER, "session_override_01")
            .body(Body::from(
                json!({
                    "model": "claude-sonnet-4-6",
                    "messages": [{"role":"user","content":"override"}]
                })
                .to_string(),
            ))
            .expect("request");

        let response = app.oneshot(request).await.expect("response");
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .expect("request id");

        let row = sqlx::query("SELECT project_id, session_id FROM requests WHERE id = ?1")
            .bind(request_id)
            .fetch_one(store.pool())
            .await
            .expect("request row");
        assert_eq!(row.get::<String, _>("project_id"), "custom-project-name");
        assert_eq!(row.get::<String, _>("session_id"), "session_override_01");
    }
}
