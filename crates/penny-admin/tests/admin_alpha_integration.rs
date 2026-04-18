use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use chrono::{Duration, Utc};
use penny_admin::{build_router, AdminState};
use penny_config::LoopAction;
use penny_detect::{DetectEngine, DetectorConfig};
use penny_store::{BudgetRepo, SqliteStore};
use penny_types::{Budget, Money, RequestDigest, ScopeType, WindowType};
use serde_json::{json, Value};
use sqlx::query;
use tower::ServiceExt;

async fn setup_store() -> SqliteStore {
    SqliteStore::connect("sqlite::memory:")
        .await
        .expect("create store")
}

#[tokio::test]
async fn admin_estimate_and_report_summary_work_integration_flow() {
    let store = setup_store().await;

    let budget = Budget {
        id: 0,
        scope_type: ScopeType::Global,
        scope_id: "*".to_string(),
        window_type: WindowType::Day,
        hard_limit_usd: Some(Money::from_usd(20.0).expect("money")),
        soft_limit_usd: Some(Money::from_usd(10.0).expect("money")),
        action_on_hard: "block".to_string(),
        action_on_soft: "warn".to_string(),
        preset_source: Some("integration".to_string()),
    };
    store.upsert(&budget).await.expect("upsert budget");

    query(
        r#"
        INSERT INTO providers (id, name, base_url, api_format, enabled)
        VALUES ('anthropic', 'Anthropic', 'https://api.anthropic.com', 'anthropic', 1)
        "#,
    )
    .execute(store.pool())
    .await
    .expect("insert provider");
    query(
        r#"
        INSERT INTO models (id, provider_id, external_name, display_name, class)
        VALUES ('claude-sonnet-4-6', 'anthropic', 'claude-sonnet-4-6', 'Claude Sonnet 4.6', 'balanced')
        "#,
    )
    .execute(store.pool())
    .await
    .expect("insert model");
    query(
        r#"
        INSERT INTO pricebook_entries (
            model_id, input_per_mtok, output_per_mtok, input_per_mtok_micros, output_per_mtok_micros, effective_from, source
        )
        VALUES ('claude-sonnet-4-6', 3.0, 15.0, 3000000, 15000000, datetime('now', '-1 day'), 'integration')
        "#,
    )
    .execute(store.pool())
    .await
    .expect("insert pricebook");

    let app = build_router(AdminState::new(store));

    let estimate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/estimate")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-6",
                        "task_type": "single_pass",
                        "context_tokens": 1000
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("estimate response");
    assert_eq!(estimate_response.status(), StatusCode::OK);
    let body = to_bytes(estimate_response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(payload["model"], "claude-sonnet-4-6");
    assert_eq!(payload["task_type"], "single_pass");
    assert_eq!(payload["range"]["confidence"], "high");
    assert_eq!(payload["budget"]["status"], "within_limit");
    assert_eq!(payload["route_preview"]["provider_id"], "anthropic");
}

#[tokio::test]
async fn admin_detect_status_and_resume_work_integration_flow() {
    let store = setup_store().await;
    let detector = Arc::new(DetectEngine::new(DetectorConfig {
        enabled: true,
        burn_rate_alert_usd_per_hour: 9999.0,
        loop_window_seconds: 120,
        loop_threshold_similar_requests: 2,
        loop_action: LoopAction::Pause,
    }));
    let start = Utc::now();
    let digest = |at| RequestDigest {
        model: "claude-sonnet-4-6".to_string(),
        input_tokens: 100,
        cost_usd: Money::from_usd(0.1).expect("money"),
        tool_name: None,
        tool_succeeded: true,
        content_hash: 123,
        timestamp: at,
    };
    detector.feed("sess-integration", Some("req-a"), digest(start));
    detector.feed(
        "sess-integration",
        Some("req-b"),
        digest(start + Duration::seconds(2)),
    );

    let app = build_router(AdminState::new(store).with_detector(detector));

    let status_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/detect/status")
                .body(Body::empty())
                .expect("status request"),
        )
        .await
        .expect("status response");
    assert_eq!(status_response.status(), StatusCode::OK);
    let body = to_bytes(status_response.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload["paused_sessions"][0]["session_id"],
        "sess-integration"
    );
    assert_eq!(
        payload["paused_sessions"][0]["reason"],
        "session_paused_loop_detected"
    );

    let resume_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/detect/resume")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "sess-integration",
                        "request_id": "req-resume"
                    })
                    .to_string(),
                ))
                .expect("resume request"),
        )
        .await
        .expect("resume response");
    assert_eq!(resume_response.status(), StatusCode::OK);

    let status_after_resume = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/detect/status")
                .body(Body::empty())
                .expect("status request"),
        )
        .await
        .expect("status response");
    assert_eq!(status_after_resume.status(), StatusCode::OK);
    let body = to_bytes(status_after_resume.into_body(), usize::MAX)
        .await
        .expect("body");
    let payload: Value = serde_json::from_slice(&body).expect("json");
    assert!(payload["paused_sessions"]
        .as_array()
        .expect("array")
        .is_empty());
}
