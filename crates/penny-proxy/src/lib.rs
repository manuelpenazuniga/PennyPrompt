//! Proxy plane implementation for PennyPrompt.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{Json, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Router,
};
use chrono::Utc;
use penny_cost::estimate_tokens;
use penny_providers::{MockProvider, MockProviderConfig, ProviderAdapter, ProviderError};
use penny_store::{NewRequest, ProjectRepo, RequestRepo, SqliteStore, UsageRecord};
use penny_types::{NormalizedRequest, ResponseBody, UsageSource};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::net::TcpListener;
use uuid::Uuid;

pub const DEFAULT_PROXY_BIND: &str = "127.0.0.1:8585";
const REQUEST_ID_HEADER: &str = "x-penny-request-id";

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
    default_project_id: String,
    default_session_id: String,
}

impl ProxyState {
    pub fn with_provider(provider: Arc<dyn ProviderAdapter>, models: Vec<String>) -> Self {
        Self {
            provider,
            store: None,
            models,
            default_project_id: "default".to_string(),
            default_session_id: "session-auto".to_string(),
        }
    }

    pub fn with_store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    pub fn mock_default() -> Self {
        let mock = MockProvider::new(MockProviderConfig::default());
        let models = mock.config().supported_models.clone();
        Self::with_provider(Arc::new(mock), models)
    }
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

#[derive(Debug, Clone)]
struct NormalizedUsage {
    input_tokens: u64,
    output_tokens: u64,
    source: UsageSource,
    pricing_snapshot: Value,
}

pub fn build_router(state: ProxyState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(post_chat_completions))
        .route("/v1/models", get(get_models))
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
    Json(payload): Json<ChatCompletionsRequest>,
) -> Response {
    if let Err(message) = validate_chat_request(&payload) {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request", message);
    }

    let normalized = normalize_chat_request(&ctx, &state, &payload);

    let provider_response = match state.provider.send(normalized.clone()).await {
        Ok(response) => response,
        Err(ProviderError::UnsupportedModel(model)) => {
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
            if let Err(message) = persist_request_and_usage(&state, &normalized, &usage).await {
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
                if let Err(message) = persist_request_and_usage(&state, &normalized, &usage).await {
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

async fn persist_request_and_usage(
    state: &ProxyState,
    normalized: &NormalizedRequest,
    usage: &NormalizedUsage,
) -> Result<(), String> {
    let Some(store) = &state.store else {
        return Ok(());
    };

    let project_id = ProjectRepo::upsert_by_path(store, &state.default_project_id)
        .await
        .map_err(|err| err.to_string())?;

    let request = NewRequest {
        id: normalized.id.clone(),
        session_id: None,
        project_id,
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
        cost_usd: 0.0,
        source: usage.source.clone(),
        pricing_snapshot: usage.pricing_snapshot.clone(),
    };
    RequestRepo::insert_usage(store, &usage_record)
        .await
        .map_err(|err| err.to_string())?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use sqlx::Row;
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
            "SELECT id, model_requested, model_used, is_streaming, started_at FROM requests WHERE id = ?1",
        )
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("request row");
        assert_eq!(row.get::<String, _>("id"), request_id);
        assert_eq!(row.get::<String, _>("model_requested"), "claude-sonnet-4-6");
        assert_eq!(row.get::<String, _>("model_used"), "claude-sonnet-4-6");
        assert_eq!(row.get::<i64, _>("is_streaming"), 0);

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
}
