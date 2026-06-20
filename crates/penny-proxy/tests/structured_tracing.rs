use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use penny_proxy::{build_router, ProxyState};
use serde_json::json;
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Record};
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

const SESSION_OVERRIDE_HEADER: &str = "x-penny-session";

#[derive(Clone, Default)]
struct CapturedLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone)]
struct FieldMap(BTreeMap<String, String>);

impl Visit for FieldMap {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        self.0.insert(
            field.name().to_string(),
            rendered.trim_matches('"').to_string(),
        );
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S> Layer<S> for CapturedLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = FieldMap::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut fields = FieldMap::default();
        values.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if let Some(existing) = extensions.get_mut::<FieldMap>() {
                existing.0.extend(fields.0);
            } else {
                extensions.insert(fields);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(span_fields) = span.extensions().get::<FieldMap>() {
                    for (key, value) in &span_fields.0 {
                        fields.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }

        let mut event_fields = FieldMap::default();
        event.record(&mut event_fields);
        fields.extend(event_fields.0);

        self.events
            .lock()
            .expect("captured event lock")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields,
            });
    }
}

fn global_trace_capture() -> CapturedLayer {
    static LAYER: OnceLock<CapturedLayer> = OnceLock::new();
    LAYER
        .get_or_init(|| {
            let layer = CapturedLayer::default();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_subscriber::filter::LevelFilter::TRACE)
                .with(layer.clone());
            tracing::subscriber::set_global_default(subscriber)
                .expect("install test tracing subscriber");
            layer
        })
        .clone()
}

fn clear_captured_events(layer: &CapturedLayer) {
    layer.events.lock().expect("captured event lock").clear();
}

fn captured_events(layer: &CapturedLayer) -> Vec<CapturedEvent> {
    layer.events.lock().expect("captured event lock").clone()
}

fn event_with_message<'a>(
    events: &'a [CapturedEvent],
    target: &str,
    message: &str,
) -> &'a CapturedEvent {
    events
        .iter()
        .find(|event| {
            event.target == target
                && event
                    .fields
                    .get("message")
                    .is_some_and(|value| value.contains(message))
        })
        .unwrap_or_else(|| panic!("missing event target={target} message={message}: {events:?}"))
}

fn assert_success_span_fields(event: &CapturedEvent, session_id: &str) {
    assert!(event
        .fields
        .get("request_id")
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(
        event.fields.get("session_id").map(String::as_str),
        Some(session_id)
    );
    assert_eq!(
        event.fields.get("model").map(String::as_str),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        event.fields.get("provider").map(String::as_str),
        Some("mock")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn structured_tracing_emits_success_hot_path_events() {
    let layer = global_trace_capture();
    clear_captured_events(&layer);
    let trace_session_id = "trace-session-success";
    let response = build_router(ProxyState::mock_default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SESSION_OVERRIDE_HEADER, trace_session_id)
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-6",
                        "messages": [{"role": "user", "content": "hello"}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let events = captured_events(&layer);
    let request_events = events
        .iter()
        .filter(|event| {
            event.fields.get("session_id").map(String::as_str) == Some(trace_session_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let received = event_with_message(&request_events, "proxy.request", "received");
    let completed = event_with_message(&request_events, "proxy.completion", "completed");
    let reconciled = event_with_message(&request_events, "proxy.ledger", "reconciled");

    assert_success_span_fields(received, trace_session_id);
    assert_success_span_fields(completed, trace_session_id);
    assert_success_span_fields(reconciled, trace_session_id);
    assert_eq!(
        completed.fields.get("input_tokens").map(String::as_str),
        Some("120")
    );
    assert_eq!(
        completed.fields.get("output_tokens").map(String::as_str),
        Some("48")
    );
    assert!(completed.fields.contains_key("cost_usd"));
    assert!(completed.fields.contains_key("latency_ms"));
    assert!(reconciled.fields.contains_key("reconciled_usd"));
    assert_eq!(
        reconciled.fields.get("source").map(String::as_str),
        Some("Provider")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn structured_tracing_emits_one_error_event_for_invalid_request() {
    let layer = global_trace_capture();
    clear_captured_events(&layer);
    let trace_model = "trace-invalid-model";
    let response = build_router(ProxyState::mock_default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": trace_model,
                        "messages": "invalid"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let events = captured_events(&layer);
    let errors = events
        .iter()
        .filter(|event| {
            event.target == "proxy.error"
                && event.fields.get("model").map(String::as_str) == Some(trace_model)
        })
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "unexpected error events: {events:?}");
    let error = errors[0];
    assert_eq!(
        error.fields.get("tag").map(String::as_str),
        Some("invalid_request")
    );
    assert_eq!(error.fields.get("status").map(String::as_str), Some("400"));
    assert!(error
        .fields
        .get("request_id")
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(
        error.fields.get("provider").map(String::as_str),
        Some("mock")
    );
}
