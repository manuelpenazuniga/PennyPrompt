//! Proxy plane implementation for PennyPrompt.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use axum::{
    extract::{Json, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Router,
};
use chrono::Utc;
use penny_budget::BudgetEvaluator;
use penny_config::{
    AppConfig as RuntimeConfig, BudgetConfig as RuntimeBudgetConfig, ScopeType as ConfigScopeType,
    WindowType as ConfigWindowType,
};
use penny_cost::{estimate_tokens, PricingEngine};
use penny_ledger::CostLedger;
use penny_providers::{MockProvider, MockProviderConfig, ProviderAdapter, ProviderError};
use penny_store::{
    BudgetRepo, NewRequest, ProjectRepo, RequestRepo, SessionRepo, SqliteStore, UsageRecord,
};
use penny_types::{
    Budget, BudgetBlockDetail, Mode, NormalizedRequest, ResponseBody, RouteDecision, ScopeType,
    UsageSource, WindowType,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{query, query_scalar};
use thiserror::Error;
use tokio::net::TcpListener;
use uuid::Uuid;

pub const DEFAULT_PROXY_BIND: &str = "127.0.0.1:8585";
const REQUEST_ID_HEADER: &str = "x-penny-request-id";
const PROJECT_OVERRIDE_HEADER: &str = "x-penny-project";
const SESSION_OVERRIDE_HEADER: &str = "x-penny-session";
const CWD_OVERRIDE_HEADER: &str = "x-penny-cwd";

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
    store: Option<SqliteStore>,
    models: Vec<String>,
    mode: Mode,
    default_project_id: String,
    default_session_id: String,
    session_window_minutes: u64,
    started_at: Instant,
}

impl ProxyState {
    pub fn with_provider(provider: Arc<dyn ProviderAdapter>, models: Vec<String>) -> Self {
        Self {
            provider,
            store: None,
            models,
            mode: Mode::Guard,
            default_project_id: "default".to_string(),
            default_session_id: "session-auto".to_string(),
            session_window_minutes: 30,
            started_at: Instant::now(),
        }
    }

    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
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
        .with_mode(mode_from_config(&config.server.mode))
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
    estimated_cost_usd: f64,
    reserve_persisted: bool,
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
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "attribution_failed",
                message,
            );
        }
    };
    normalized.project_id = project_id;
    normalized.session_id = session_id;

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
    };

    let status = StatusCode::from_u16(provider_response.status).unwrap_or(StatusCode::BAD_GATEWAY);
    match provider_response.body {
        ResponseBody::Complete(value) => {
            let usage = provider_usage_from_completion(&value)
                .unwrap_or_else(|| estimated_usage_from_request(&normalized));
            let usage_cost_usd = actual_usage_cost_usd(&state, &normalized, &usage)
                .await
                .unwrap_or_else(|_| {
                    enforcement
                        .as_ref()
                        .map(|ctx| ctx.estimated_cost_usd)
                        .unwrap_or(0.0)
                });
            if let Some(context) = enforcement {
                if let Err(message) = reconcile_after_dispatch(
                    &state,
                    &normalized.id,
                    usage_cost_usd,
                    context.reserve_persisted,
                )
                .await
                {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "ledger_reconcile_failed",
                        message,
                    );
                }
            }
            if let Err(message) =
                persist_request_and_usage(&state, &normalized, &usage, usage_cost_usd).await
            {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "persistence_failed",
                    message,
                );
            }

            (status, Json(value)).into_response()
        }
        ResponseBody::Stream(_) => {
            if let Some(lines) = state.provider.stream_response_lines(&normalized) {
                let usage = provider_usage_from_sse_lines(&lines)
                    .unwrap_or_else(|| estimated_usage_from_request(&normalized));
                let usage_cost_usd = actual_usage_cost_usd(&state, &normalized, &usage)
                    .await
                    .unwrap_or_else(|_| {
                        enforcement
                            .as_ref()
                            .map(|ctx| ctx.estimated_cost_usd)
                            .unwrap_or(0.0)
                    });
                if let Some(context) = enforcement {
                    if let Err(message) = reconcile_after_dispatch(
                        &state,
                        &normalized.id,
                        usage_cost_usd,
                        context.reserve_persisted,
                    )
                    .await
                    {
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "ledger_reconcile_failed",
                            message,
                        );
                    }
                }
                if let Err(message) =
                    persist_request_and_usage(&state, &normalized, &usage, usage_cost_usd).await
                {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "persistence_failed",
                        message,
                    );
                }

                let mut response = (status, lines.concat()).into_response();
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream; charset=utf-8"),
                );
                response
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

async fn get_internal_health(State(state): State<Arc<ProxyState>>) -> Response {
    let (db_status, db_error, status_code) = match &state.store {
        None => ("disabled", None, StatusCode::OK),
        Some(store) => match query("SELECT 1").fetch_one(store.pool()).await {
            Ok(_) => ("up", None, StatusCode::OK),
            Err(err) => (
                "down",
                Some(err.to_string()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
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
    for budget in &config.budgets {
        let mapped = map_config_budget(budget);
        let stored = BudgetRepo::upsert(store, &mapped)
            .await
            .map_err(|err| err.to_string())?;
        seeded.push(stored);
    }
    Ok(seeded)
}

fn map_config_budget(budget: &RuntimeBudgetConfig) -> Budget {
    Budget {
        id: 0,
        scope_type: scope_type_from_config(&budget.scope_type),
        scope_id: budget.scope_id.clone(),
        window_type: window_type_from_config(&budget.window_type),
        hard_limit_usd: budget.hard_limit_usd,
        soft_limit_usd: budget.soft_limit_usd,
        action_on_hard: budget.action_on_hard.clone(),
        action_on_soft: budget.action_on_soft.clone(),
        preset_source: budget.preset_source.clone(),
    }
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
                return Err(budget_error_response(
                    "budget_engine_failure",
                    format!("budget engine failed before dispatch: {message}"),
                    None,
                ));
            }
            false
        }
    };

    if !has_budgets {
        return Ok(Some(BudgetEnforcementContext {
            estimated_cost_usd: 0.0,
            reserve_persisted: false,
        }));
    }

    let estimated_cost_usd = match estimated_request_cost_usd(state, request).await {
        Ok(cost) => cost,
        Err(message) => {
            if matches!(state.mode, Mode::Guard) {
                return Err(budget_error_response(
                    "budget_engine_failure",
                    format!("budget engine failed before dispatch: {message}"),
                    None,
                ));
            }
            0.0
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
                "{} ({:?}) exceeded: {:.6} / {:.6} - {reason}",
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
                Err(budget_error_response("budget_engine_failure", reason, None))
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
) -> Result<f64, String> {
    let Some(store) = &state.store else {
        return Ok(0.0);
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
) -> Result<f64, String> {
    let Some(store) = &state.store else {
        return Ok(0.0);
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
    usage_cost_usd: f64,
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
    cost_usd: f64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use penny_budget::BudgetEvaluator;
    use penny_config::{load_config, LoadOptions};
    use penny_cost::import_pricebook_files;
    use penny_store::BudgetRepo;
    use penny_types::{Budget, RouteDecision, ScopeType, WindowType};
    use sqlx::Row;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
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

    async fn seed_global_day_budget(store: &SqliteStore, hard_limit_usd: f64) {
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
        assert_eq!(all[0].hard_limit_usd, Some(7.0));
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
            .evaluate(&budget_request("req_seeded_eval"), 11.0)
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
        assert_eq!(json_body["error"]["code"], "invalid_request");
        assert_eq!(
            json_body["error"]["message"],
            "field `model` must not be empty"
        );
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
        seed_global_day_budget(&store, 0.0).await;

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
    async fn successful_dispatch_persists_reserve_then_reconcile() {
        let (app, store) = app_with_store().await;
        seed_pricebooks(&store).await;
        seed_global_day_budget(&store, 10.0).await;

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
        seed_global_day_budget(&store, 10.0).await;

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
