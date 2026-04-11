//! Provider adapter abstractions.

use async_trait::async_trait;
use penny_types::{NormalizedRequest, ProviderResponse, ResponseBody, StreamDescriptor};
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("model `{0}` is not supported by provider")]
    UnsupportedModel(String),
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse, ProviderError>;
    fn provider_id(&self) -> &str;
    fn supports_model(&self, model: &str) -> bool;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use penny_types::ResponseBody;

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
        match err {
            ProviderError::UnsupportedModel(model) => assert_eq!(model, "unknown-model"),
        }
    }
}
