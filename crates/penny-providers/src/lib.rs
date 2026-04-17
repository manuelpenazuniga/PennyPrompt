//! Provider adapter abstractions.

use async_trait::async_trait;
use penny_types::{NormalizedRequest, ProviderResponse, ResponseBody, StreamDescriptor};
use reqwest::{header::CONTENT_TYPE, Client, StatusCode};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("model `{0}` is not supported by provider")]
    UnsupportedModel(String),
    #[error("provider internal state is unavailable: {0}")]
    InternalState(String),
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError>;
    fn provider_id(&self) -> &str;
    fn supports_model(&self, model: &str) -> bool;
    fn stream_response_lines(&self, _req: &NormalizedRequest) -> Option<Vec<String>> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiProviderConfig {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: String,
    pub supported_models: Vec<String>,
    pub timeout_ms: u64,
}

impl Default for OpenAiProviderConfig {
    fn default() -> Self {
        Self {
            provider_id: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: String::new(),
            supported_models: vec!["gpt-4.1".to_string()],
            timeout_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    config: OpenAiProviderConfig,
    client: Client,
    pending_streams: ArcStreamCache,
}

impl OpenAiProvider {
    pub fn new(config: OpenAiProviderConfig) -> Result<Self, String> {
        let timeout = std::time::Duration::from_millis(config.timeout_ms.max(1));
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self {
            config,
            client,
            pending_streams: std::sync::Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn endpoint_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    fn openai_headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("authorization", format!("Bearer {}", self.config.api_key))
            .header(CONTENT_TYPE, "application/json")
    }

    fn build_payload(&self, req: &NormalizedRequest) -> Value {
        json!({
            "model": req.model_resolved,
            "messages": req.messages,
            "stream": req.stream,
        })
    }

    fn map_error_response(&self, status: StatusCode, body: &str) -> Value {
        match serde_json::from_str::<Value>(body) {
            Ok(parsed) if parsed.get("error").is_some() => parsed,
            _ => json!({
                "error": {
                    "message": if body.trim().is_empty() {
                        format!("upstream request failed with status {}", status.as_u16())
                    } else {
                        body.trim().to_string()
                    },
                    "type": "provider_error",
                    "code": status.as_u16(),
                }
            }),
        }
    }

    fn normalize_sse_lines(&self, raw_body: &str) -> Vec<String> {
        let mut lines = raw_body
            .split("\n\n")
            .filter_map(|segment| {
                let trimmed = segment.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(format!("{trimmed}\n\n"))
            })
            .collect::<Vec<_>>();

        if lines.is_empty() && !raw_body.trim().is_empty() {
            lines.push(format!("{}\n\n", raw_body.trim()));
        }
        lines
    }

    fn lock_streams(&self) -> Result<MutexGuard<'_, HashMap<String, Vec<String>>>, ProviderError> {
        self.pending_streams
            .lock()
            .map_err(|err| ProviderError::InternalState(err.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct AnthropicProviderConfig {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: String,
    pub api_version: String,
    pub supported_models: Vec<String>,
    pub timeout_ms: u64,
}

impl Default for AnthropicProviderConfig {
    fn default() -> Self {
        Self {
            provider_id: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            api_version: "2023-06-01".to_string(),
            supported_models: vec!["claude-sonnet-4-6".to_string()],
            timeout_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    config: AnthropicProviderConfig,
    client: Client,
    pending_streams: ArcStreamCache,
}

type ArcStreamCache = std::sync::Arc<Mutex<HashMap<String, Vec<String>>>>;

#[derive(Debug, Default)]
struct AnthropicStreamState {
    message_id: Option<String>,
    model: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    finish_reason: Option<String>,
    deltas: Vec<String>,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicProviderConfig) -> Result<Self, String> {
        let timeout = std::time::Duration::from_millis(config.timeout_ms.max(1));
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self {
            config,
            client,
            pending_streams: std::sync::Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn endpoint_url(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }

    fn anthropic_headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.api_version)
            .header(CONTENT_TYPE, "application/json")
    }

    fn build_payload(&self, req: &NormalizedRequest) -> Value {
        let mut system_segments: Vec<String> = Vec::new();
        let mut anthropic_messages: Vec<Value> = Vec::new();
        if let Some(messages) = req.messages.as_array() {
            for message in messages {
                let role = message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
                let text = extract_message_text(message.get("content").unwrap_or(&Value::Null))
                    .unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                if role == "system" {
                    system_segments.push(text);
                    continue;
                }
                let normalized_role = match role {
                    "assistant" => "assistant",
                    _ => "user",
                };
                anthropic_messages.push(json!({
                    "role": normalized_role,
                    "content": [{"type": "text", "text": text}],
                }));
            }
        }

        let mut payload = json!({
            "model": req.model_resolved,
            "messages": anthropic_messages,
            "max_tokens": req.estimated_output_tokens.max(1),
            "stream": req.stream,
        });
        if !system_segments.is_empty() {
            payload["system"] = Value::String(system_segments.join("\n"));
        }
        payload
    }

    fn map_non_stream_response(&self, req: &NormalizedRequest, payload: &Value) -> Value {
        let message_id = payload
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("msg_{}", req.id));
        let model = payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&req.model_resolved)
            .to_string();
        let content = payload
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let usage = payload.get("usage").cloned().unwrap_or_else(|| json!({}));
        let prompt_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let completion_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let finish_reason = map_finish_reason(
            payload
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn"),
        );

        json!({
            "id": message_id,
            "object": "chat.completion",
            "created": req.timestamp.timestamp(),
            "model": model,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": content,
                },
                "finish_reason": finish_reason,
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
            },
        })
    }

    fn map_error_response(&self, status: StatusCode, body: &str) -> Value {
        let parsed: Option<Value> = serde_json::from_str(body).ok();
        let message = parsed
            .as_ref()
            .and_then(|json| json.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(body)
            .trim()
            .to_string();
        let error_type = parsed
            .as_ref()
            .and_then(|json| json.get("error"))
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("provider_error")
            .to_string();
        json!({
            "error": {
                "message": if message.is_empty() { format!("upstream request failed with status {}", status.as_u16()) } else { message },
                "type": error_type,
                "code": status.as_u16(),
            }
        })
    }

    fn map_stream_lines(&self, req: &NormalizedRequest, raw_body: &str) -> Vec<String> {
        let mut state = AnthropicStreamState::default();
        for block in split_sse_blocks(raw_body) {
            let event = parse_sse_event(block);
            if event.data.eq_ignore_ascii_case("[done]") {
                continue;
            }
            let parsed: Value = match serde_json::from_str(&event.data) {
                Ok(value) => value,
                Err(_) => continue,
            };
            match event.name.as_deref() {
                Some("message_start") => {
                    let message = parsed.get("message").unwrap_or(&Value::Null);
                    state.message_id = message
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    state.model = message
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    state.input_tokens = message
                        .get("usage")
                        .and_then(|usage| usage.get("input_tokens"))
                        .and_then(Value::as_u64)
                        .or(state.input_tokens);
                    state.output_tokens = message
                        .get("usage")
                        .and_then(|usage| usage.get("output_tokens"))
                        .and_then(Value::as_u64)
                        .or(state.output_tokens);
                }
                Some("content_block_start") => {
                    if let Some(text) = parsed
                        .get("content_block")
                        .and_then(|block| block.get("text"))
                        .and_then(Value::as_str)
                    {
                        if !text.is_empty() {
                            state.deltas.push(text.to_string());
                        }
                    }
                }
                Some("content_block_delta") => {
                    if let Some(text) = parsed
                        .get("delta")
                        .and_then(|delta| delta.get("text"))
                        .and_then(Value::as_str)
                    {
                        if !text.is_empty() {
                            state.deltas.push(text.to_string());
                        }
                    }
                }
                Some("message_delta") => {
                    state.output_tokens = parsed
                        .get("usage")
                        .and_then(|usage| usage.get("output_tokens"))
                        .and_then(Value::as_u64)
                        .or(state.output_tokens);
                    state.finish_reason = parsed
                        .get("delta")
                        .and_then(|delta| delta.get("stop_reason"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or(state.finish_reason);
                }
                Some("message_stop") => {
                    state.finish_reason = state
                        .finish_reason
                        .take()
                        .or_else(|| Some("end_turn".to_string()));
                }
                _ => {}
            }
        }

        build_openai_sse_lines(req, &state)
    }

    fn lock_streams(&self) -> Result<MutexGuard<'_, HashMap<String, Vec<String>>>, ProviderError> {
        self.pending_streams
            .lock()
            .map_err(|err| ProviderError::InternalState(err.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct MockProviderConfig {
    pub provider_id: String,
    pub supported_models: Vec<String>,
    pub completion_text: String,
    pub usage: MockUsage,
    pub upstream_ms: u64,
    pub stream_format: String,
}

impl Default for MockProviderConfig {
    fn default() -> Self {
        Self {
            provider_id: "mock".to_string(),
            supported_models: vec![
                "mock-sonnet".to_string(),
                "claude-sonnet-4-6".to_string(),
                "gpt-4.1".to_string(),
            ],
            completion_text: "Mock provider deterministic response.".to_string(),
            usage: MockUsage {
                input_tokens: 120,
                output_tokens: 48,
            },
            upstream_ms: 42,
            stream_format: "sse".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MockProvider {
    config: MockProviderConfig,
}

impl MockProvider {
    pub fn new(config: MockProviderConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &MockProviderConfig {
        &self.config
    }

    pub fn completion_payload(&self, req: &NormalizedRequest) -> Value {
        json!({
            "id": format!("chatcmpl_mock_{}", req.id),
            "object": "chat.completion",
            "created": req.timestamp.timestamp(),
            "model": req.model_resolved,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": self.config.completion_text
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": self.config.usage.input_tokens,
                "completion_tokens": self.config.usage.output_tokens,
                "total_tokens": self.config.usage.input_tokens + self.config.usage.output_tokens
            }
        })
    }

    /// Deterministic SSE lines used by integration tests to simulate streaming.
    pub fn stream_sse_lines(&self, req: &NormalizedRequest) -> Vec<String> {
        let first_chunk = json!({
            "id": format!("chatcmpl_mock_{}", req.id),
            "object": "chat.completion.chunk",
            "created": req.timestamp.timestamp(),
            "model": req.model_resolved,
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant"},
                "finish_reason": Value::Null
            }]
        });

        let second_chunk = json!({
            "id": format!("chatcmpl_mock_{}", req.id),
            "object": "chat.completion.chunk",
            "created": req.timestamp.timestamp(),
            "model": req.model_resolved,
            "choices": [{
                "index": 0,
                "delta": {"content": self.config.completion_text},
                "finish_reason": Value::Null
            }]
        });

        let final_chunk = json!({
            "id": format!("chatcmpl_mock_{}", req.id),
            "object": "chat.completion.chunk",
            "created": req.timestamp.timestamp(),
            "model": req.model_resolved,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": self.config.usage.input_tokens,
                "completion_tokens": self.config.usage.output_tokens,
                "total_tokens": self.config.usage.input_tokens + self.config.usage.output_tokens
            }
        });

        vec![
            format!("data: {first_chunk}\n\n"),
            format!("data: {second_chunk}\n\n"),
            format!("data: {final_chunk}\n\n"),
            "data: [DONE]\n\n".to_string(),
        ]
    }
}

#[async_trait]
impl ProviderAdapter for MockProvider {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
        if !self.supports_model(&req.model_resolved) && !self.supports_model(&req.model_requested) {
            return Err(ProviderError::UnsupportedModel(req.model_requested));
        }

        let body = if req.stream {
            ResponseBody::Stream(StreamDescriptor {
                provider: self.config.provider_id.clone(),
                format: self.config.stream_format.clone(),
            })
        } else {
            ResponseBody::Complete(self.completion_payload(&req))
        };

        Ok(ProviderResponse {
            status: 200,
            body,
            upstream_ms: self.config.upstream_ms,
        })
    }

    fn provider_id(&self) -> &str {
        &self.config.provider_id
    }

    fn supports_model(&self, model: &str) -> bool {
        self.config.supported_models.iter().any(|m| m == model)
    }

    fn stream_response_lines(&self, req: &NormalizedRequest) -> Option<Vec<String>> {
        Some(self.stream_sse_lines(req))
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicProvider {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
        if !self.supports_model(&req.model_resolved) && !self.supports_model(&req.model_requested) {
            return Err(ProviderError::UnsupportedModel(req.model_requested));
        }

        let payload = self.build_payload(&req);
        let request = self.client.post(self.endpoint_url()).json(&payload);
        let request = self.anthropic_headers(request);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ProviderResponse {
                    status: StatusCode::BAD_GATEWAY.as_u16(),
                    body: ResponseBody::Complete(json!({
                        "error": {
                            "message": error.to_string(),
                            "type": "upstream_unavailable",
                            "code": StatusCode::BAD_GATEWAY.as_u16(),
                        }
                    })),
                    upstream_ms: 0,
                });
            }
        };
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Ok(ProviderResponse {
                status: status.as_u16(),
                body: ResponseBody::Complete(self.map_error_response(status, &body_text)),
                upstream_ms: 0,
            });
        }

        if req.stream {
            let lines = self.map_stream_lines(&req, &body_text);
            let mut streams = self.lock_streams()?;
            streams.insert(req.id.clone(), lines);
            return Ok(ProviderResponse {
                status: status.as_u16(),
                body: ResponseBody::Stream(StreamDescriptor {
                    provider: self.provider_id().to_string(),
                    format: "sse".to_string(),
                }),
                upstream_ms: 0,
            });
        }

        let parsed: Value = serde_json::from_str(&body_text).unwrap_or_else(|_| json!({}));
        Ok(ProviderResponse {
            status: status.as_u16(),
            body: ResponseBody::Complete(self.map_non_stream_response(&req, &parsed)),
            upstream_ms: 0,
        })
    }

    fn provider_id(&self) -> &str {
        &self.config.provider_id
    }

    fn supports_model(&self, model: &str) -> bool {
        if self.config.supported_models.iter().any(|m| m == model) {
            return true;
        }
        model.starts_with("claude")
    }

    fn stream_response_lines(&self, req: &NormalizedRequest) -> Option<Vec<String>> {
        let mut streams = self.lock_streams().ok()?;
        streams.remove(&req.id)
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiProvider {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError> {
        if !self.supports_model(&req.model_resolved) && !self.supports_model(&req.model_requested) {
            return Err(ProviderError::UnsupportedModel(req.model_requested));
        }

        let payload = self.build_payload(&req);
        let request = self.client.post(self.endpoint_url()).json(&payload);
        let request = self.openai_headers(request);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ProviderResponse {
                    status: StatusCode::BAD_GATEWAY.as_u16(),
                    body: ResponseBody::Complete(json!({
                        "error": {
                            "message": error.to_string(),
                            "type": "upstream_unavailable",
                            "code": StatusCode::BAD_GATEWAY.as_u16(),
                        }
                    })),
                    upstream_ms: 0,
                });
            }
        };
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Ok(ProviderResponse {
                status: status.as_u16(),
                body: ResponseBody::Complete(self.map_error_response(status, &body_text)),
                upstream_ms: 0,
            });
        }

        if req.stream {
            let lines = self.normalize_sse_lines(&body_text);
            let mut streams = self.lock_streams()?;
            streams.insert(req.id.clone(), lines);
            return Ok(ProviderResponse {
                status: status.as_u16(),
                body: ResponseBody::Stream(StreamDescriptor {
                    provider: self.provider_id().to_string(),
                    format: "sse".to_string(),
                }),
                upstream_ms: 0,
            });
        }

        let parsed = serde_json::from_str::<Value>(&body_text).unwrap_or_else(|_| json!({}));
        Ok(ProviderResponse {
            status: status.as_u16(),
            body: ResponseBody::Complete(parsed),
            upstream_ms: 0,
        })
    }

    fn provider_id(&self) -> &str {
        &self.config.provider_id
    }

    fn supports_model(&self, model: &str) -> bool {
        if self.config.supported_models.iter().any(|m| m == model) {
            return true;
        }
        model.starts_with("gpt-")
            || model.starts_with("o1")
            || model.starts_with("o3")
            || model.starts_with("omni")
    }

    fn stream_response_lines(&self, req: &NormalizedRequest) -> Option<Vec<String>> {
        let mut streams = self.lock_streams().ok()?;
        streams.remove(&req.id)
    }
}

#[derive(Debug, Default)]
struct SseEvent {
    name: Option<String>,
    data: String,
}

fn split_sse_blocks(raw_body: &str) -> Vec<&str> {
    raw_body
        .split("\n\n")
        .filter(|chunk| !chunk.trim().is_empty())
        .collect()
}

fn parse_sse_event(raw_event: &str) -> SseEvent {
    let mut event = SseEvent::default();
    let mut data_lines = Vec::new();
    for line in raw_event.lines() {
        if let Some(name) = line.strip_prefix("event:") {
            event.name = Some(name.trim().to_string());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim().to_string());
        }
    }
    event.data = data_lines.join("\n");
    event
}

fn build_openai_sse_lines(req: &NormalizedRequest, state: &AnthropicStreamState) -> Vec<String> {
    let message_id = state.message_id.as_deref().unwrap_or(&req.id).to_string();
    let model = state
        .model
        .as_deref()
        .unwrap_or(&req.model_resolved)
        .to_string();
    let finish_reason = map_finish_reason(state.finish_reason.as_deref().unwrap_or("end_turn"));
    let prompt_tokens = state.input_tokens.unwrap_or(0);
    let completion_tokens = state.output_tokens.unwrap_or(0);

    let mut lines = Vec::with_capacity(state.deltas.len() + 3);
    let first_chunk = json!({
        "id": message_id,
        "object": "chat.completion.chunk",
        "created": req.timestamp.timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant"},
            "finish_reason": Value::Null,
        }],
    });
    lines.push(format!("data: {first_chunk}\n\n"));

    for delta in &state.deltas {
        let chunk = json!({
            "id": message_id,
            "object": "chat.completion.chunk",
            "created": req.timestamp.timestamp(),
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {"content": delta},
                "finish_reason": Value::Null,
            }],
        });
        lines.push(format!("data: {chunk}\n\n"));
    }

    let final_chunk = json!({
        "id": message_id,
        "object": "chat.completion.chunk",
        "created": req.timestamp.timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
    });
    lines.push(format!("data: {final_chunk}\n\n"));
    lines.push("data: [DONE]\n\n".to_string());
    lines
}

fn extract_message_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let blocks = content.as_array()?;
    let joined = blocks
        .iter()
        .filter_map(|entry| {
            if let Some(text) = entry.get("text").and_then(Value::as_str) {
                return Some(text.to_string());
            }
            entry.as_str().map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>()
        .join("");
    Some(joined)
}

fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use penny_types::ResponseBody;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };

    fn request(stream: bool) -> NormalizedRequest {
        NormalizedRequest {
            id: "req_test_01".into(),
            project_id: "penny".into(),
            session_id: "sess_test_01".into(),
            model_requested: "claude-sonnet-4-6".into(),
            model_resolved: "claude-sonnet-4-6".into(),
            provider_id: "mock".into(),
            messages: json!([{ "role": "user", "content": "hello" }]),
            stream,
            estimated_input_tokens: 10,
            estimated_output_tokens: 5,
            timestamp: Utc
                .with_ymd_and_hms(2026, 4, 11, 1, 2, 3)
                .single()
                .expect("valid static timestamp"),
        }
    }

    #[tokio::test]
    async fn non_stream_response_is_deterministic() {
        let provider = MockProvider::new(MockProviderConfig::default());
        let req = request(false);

        let first = provider.send(req.clone()).await.expect("first response");
        let second = provider.send(req).await.expect("second response");

        assert_eq!(first, second);
        match first.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(
                    payload["choices"][0]["message"]["content"],
                    "Mock provider deterministic response."
                );
                assert_eq!(payload["usage"]["prompt_tokens"], 120);
                assert_eq!(payload["usage"]["completion_tokens"], 48);
            }
            other => panic!("expected complete response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_path_and_lines_are_deterministic() {
        let provider = MockProvider::new(MockProviderConfig::default());
        let req = request(true);

        let response = provider.send(req.clone()).await.expect("stream response");
        match response.body {
            ResponseBody::Stream(descriptor) => {
                assert_eq!(descriptor.provider, "mock");
                assert_eq!(descriptor.format, "sse");
            }
            other => panic!("expected stream response, got {other:?}"),
        }

        let first_lines = provider.stream_sse_lines(&req);
        let second_lines = provider.stream_sse_lines(&req);
        assert_eq!(first_lines, second_lines);
        assert!(first_lines.iter().any(|line| line.contains("\"usage\"")));
        assert_eq!(first_lines.last(), Some(&"data: [DONE]\n\n".to_string()));
    }

    #[tokio::test]
    async fn usage_payload_is_configurable() {
        let provider = MockProvider::new(MockProviderConfig {
            usage: MockUsage {
                input_tokens: 111,
                output_tokens: 222,
            },
            completion_text: "configured".to_string(),
            ..MockProviderConfig::default()
        });
        let response = provider.send(request(false)).await.expect("response");
        match response.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(payload["usage"]["prompt_tokens"], 111);
                assert_eq!(payload["usage"]["completion_tokens"], 222);
                assert_eq!(payload["usage"]["total_tokens"], 333);
                assert_eq!(payload["choices"][0]["message"]["content"], "configured");
            }
            other => panic!("expected complete response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_model_returns_error() {
        let provider = MockProvider::new(MockProviderConfig::default());
        let mut req = request(false);
        req.model_requested = "unknown-model".to_string();
        req.model_resolved = "unknown-model".to_string();

        let err = provider.send(req).await.expect_err("should fail");
        assert!(matches!(err, ProviderError::UnsupportedModel(model) if model == "unknown-model"));
    }

    fn anthropic_provider(base_url: String) -> AnthropicProvider {
        AnthropicProvider::new(AnthropicProviderConfig {
            base_url,
            api_key: "test-key".to_string(),
            supported_models: vec!["claude-sonnet-4-6".to_string()],
            ..AnthropicProviderConfig::default()
        })
        .expect("provider should build")
    }

    async fn spawn_single_response_server(
        status: u16,
        content_type: &'static str,
        response_body: String,
    ) -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut buffer = vec![0_u8; 64 * 1024];
            let read = stream.read(&mut buffer).await.expect("read request");
            let request_raw = String::from_utf8_lossy(&buffer[..read]).to_string();
            let _ = tx.send(request_raw);

            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                502 => "Bad Gateway",
                _ => "OK",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn anthropic_non_stream_is_mapped_to_openai_shape() {
        let (base_url, request_rx) = spawn_single_response_server(
            200,
            "application/json",
            json!({
                "id": "msg_abc",
                "model": "claude-sonnet-4-6",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Hello from Anthropic"}],
                "usage": {"input_tokens": 42, "output_tokens": 11}
            })
            .to_string(),
        )
        .await;

        let provider = anthropic_provider(base_url);
        let mut req = request(false);
        req.provider_id = "anthropic".to_string();

        let response = provider.send(req).await.expect("response");
        assert_eq!(response.status, 200);
        let request_raw = request_rx.await.expect("captured request").to_lowercase();
        assert!(request_raw.starts_with("post /v1/messages"));
        assert!(request_raw.contains("x-api-key: test-key"));
        assert!(request_raw.contains("anthropic-version: 2023-06-01"));
        match response.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(payload["object"], "chat.completion");
                assert_eq!(
                    payload["choices"][0]["message"]["content"],
                    "Hello from Anthropic"
                );
                assert_eq!(payload["usage"]["prompt_tokens"], 42);
                assert_eq!(payload["usage"]["completion_tokens"], 11);
            }
            other => panic!("expected complete response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn anthropic_streaming_sse_is_rewritten_with_usage_chunk() {
        let sse = [
            "event: message_start\n",
            "data: {\"message\":{\"id\":\"msg_stream\",\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":33,\"output_tokens\":0}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"text\":\"Hello\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"delta\":{\"text\":\" world\"}}\n\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n",
            "event: message_stop\n",
            "data: {}\n\n",
        ]
        .concat();
        let (base_url, _) = spawn_single_response_server(200, "text/event-stream", sse).await;

        let provider = anthropic_provider(base_url);
        let mut req = request(true);
        req.id = "req_stream_01".to_string();
        req.provider_id = "anthropic".to_string();

        let response = provider.send(req.clone()).await.expect("stream response");
        match response.body {
            ResponseBody::Stream(descriptor) => {
                assert_eq!(descriptor.provider, "anthropic");
                assert_eq!(descriptor.format, "sse");
            }
            other => panic!("expected stream response, got {other:?}"),
        }
        let lines = provider
            .stream_response_lines(&req)
            .expect("stream lines should exist");
        assert!(lines.iter().any(|line| line.contains("\"Hello\"")));
        assert!(lines.iter().any(|line| line.contains("\"usage\"")));
        assert_eq!(lines.last(), Some(&"data: [DONE]\n\n".to_string()));
    }

    #[tokio::test]
    async fn anthropic_error_payload_is_mapped() {
        let (base_url, _) = spawn_single_response_server(
            401,
            "application/json",
            json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "invalid x-api-key"
                }
            })
            .to_string(),
        )
        .await;

        let provider = anthropic_provider(base_url);
        let mut req = request(false);
        req.provider_id = "anthropic".to_string();

        let response = provider.send(req).await.expect("response");
        assert_eq!(response.status, 401);
        match response.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(payload["error"]["type"], "authentication_error");
                assert_eq!(payload["error"]["code"], 401);
            }
            other => panic!("expected mapped error payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn anthropic_unsupported_model_returns_error() {
        let provider = AnthropicProvider::new(AnthropicProviderConfig {
            api_key: "test-key".to_string(),
            supported_models: vec!["claude-sonnet-4-6".to_string()],
            ..AnthropicProviderConfig::default()
        })
        .expect("provider");
        let mut req = request(false);
        req.model_requested = "gpt-4.1".to_string();
        req.model_resolved = "gpt-4.1".to_string();
        req.provider_id = "anthropic".to_string();

        let error = provider.send(req).await.expect_err("unsupported model");
        assert!(matches!(error, ProviderError::UnsupportedModel(model) if model == "gpt-4.1"));
    }

    fn openai_provider(base_url: String) -> OpenAiProvider {
        OpenAiProvider::new(OpenAiProviderConfig {
            base_url,
            api_key: "test-openai-key".to_string(),
            supported_models: vec!["gpt-4.1".to_string()],
            ..OpenAiProviderConfig::default()
        })
        .expect("provider should build")
    }

    #[tokio::test]
    async fn openai_non_stream_forwards_payload_and_auth_header() {
        let (base_url, request_rx) = spawn_single_response_server(
            200,
            "application/json",
            json!({
                "id": "chatcmpl_abc",
                "object": "chat.completion",
                "created": 1_712_345_678_i64,
                "model": "gpt-4.1",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "Hello from OpenAI"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 7, "completion_tokens": 5, "total_tokens": 12}
            })
            .to_string(),
        )
        .await;

        let provider = openai_provider(base_url);
        let mut req = request(false);
        req.provider_id = "openai".to_string();
        req.model_requested = "gpt-4.1".to_string();
        req.model_resolved = "gpt-4.1".to_string();
        req.messages = json!([{ "role": "user", "content": "ping" }]);

        let response = provider.send(req).await.expect("response");
        assert_eq!(response.status, 200);
        let request_raw = request_rx.await.expect("captured request").to_lowercase();
        assert!(request_raw.starts_with("post /v1/chat/completions"));
        assert!(request_raw.contains("authorization: bearer test-openai-key"));
        assert!(request_raw.contains("\"model\":\"gpt-4.1\""));
        assert!(request_raw.contains("\"stream\":false"));
        assert!(request_raw.contains("\"role\":\"user\""));

        match response.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(
                    payload["choices"][0]["message"]["content"],
                    "Hello from OpenAI"
                );
                assert_eq!(payload["usage"]["prompt_tokens"], 7);
                assert_eq!(payload["usage"]["completion_tokens"], 5);
            }
            other => panic!("expected complete response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn openai_stream_preserves_usage_when_present() {
        let sse = [
            "data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":7}}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat();
        let (base_url, _) = spawn_single_response_server(200, "text/event-stream", sse).await;

        let provider = openai_provider(base_url);
        let mut req = request(true);
        req.id = "req_openai_stream_usage".to_string();
        req.provider_id = "openai".to_string();
        req.model_requested = "gpt-4.1".to_string();
        req.model_resolved = "gpt-4.1".to_string();

        let response = provider.send(req.clone()).await.expect("stream response");
        match response.body {
            ResponseBody::Stream(descriptor) => {
                assert_eq!(descriptor.provider, "openai");
                assert_eq!(descriptor.format, "sse");
            }
            other => panic!("expected stream response, got {other:?}"),
        }

        let lines = provider
            .stream_response_lines(&req)
            .expect("stream lines should exist");
        assert!(lines.iter().any(|line| line.contains("\"usage\"")));
        assert!(lines
            .iter()
            .any(|line| line.contains("\"content\":\"hello\"")));
        assert_eq!(lines.last(), Some(&"data: [DONE]\n\n".to_string()));
    }

    #[tokio::test]
    async fn openai_stream_can_arrive_without_usage_for_estimation_fallback() {
        let sse = [
            "data: {\"id\":\"chatcmpl_no_usage\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_no_usage\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl_no_usage\",\"object\":\"chat.completion.chunk\",\"created\":1712345678,\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat();
        let (base_url, _) = spawn_single_response_server(200, "text/event-stream", sse).await;

        let provider = openai_provider(base_url);
        let mut req = request(true);
        req.id = "req_openai_stream_no_usage".to_string();
        req.provider_id = "openai".to_string();
        req.model_requested = "gpt-4.1".to_string();
        req.model_resolved = "gpt-4.1".to_string();

        let response = provider.send(req.clone()).await.expect("stream response");
        assert!(matches!(response.body, ResponseBody::Stream(_)));
        let lines = provider
            .stream_response_lines(&req)
            .expect("stream lines should exist");
        assert!(!lines.iter().any(|line| line.contains("\"usage\"")));
        assert_eq!(lines.last(), Some(&"data: [DONE]\n\n".to_string()));
    }

    #[tokio::test]
    async fn openai_error_payload_is_mapped() {
        let (base_url, _) = spawn_single_response_server(
            401,
            "application/json",
            json!({
                "error": {
                    "message": "invalid api key",
                    "type": "invalid_request_error",
                    "code": "invalid_api_key"
                }
            })
            .to_string(),
        )
        .await;

        let provider = openai_provider(base_url);
        let mut req = request(false);
        req.provider_id = "openai".to_string();
        req.model_requested = "gpt-4.1".to_string();
        req.model_resolved = "gpt-4.1".to_string();

        let response = provider.send(req).await.expect("response");
        assert_eq!(response.status, 401);
        match response.body {
            ResponseBody::Complete(payload) => {
                assert_eq!(payload["error"]["type"], "invalid_request_error");
                assert_eq!(payload["error"]["message"], "invalid api key");
            }
            other => panic!("expected mapped error payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn openai_unsupported_model_returns_error() {
        let provider = OpenAiProvider::new(OpenAiProviderConfig {
            api_key: "test-openai-key".to_string(),
            supported_models: vec!["gpt-4.1".to_string()],
            ..OpenAiProviderConfig::default()
        })
        .expect("provider");
        let mut req = request(false);
        req.provider_id = "openai".to_string();
        req.model_requested = "claude-sonnet-4-6".to_string();
        req.model_resolved = "claude-sonnet-4-6".to_string();

        let error = provider.send(req).await.expect_err("unsupported model");
        assert!(
            matches!(error, ProviderError::UnsupportedModel(model) if model == "claude-sonnet-4-6")
        );
    }
}
