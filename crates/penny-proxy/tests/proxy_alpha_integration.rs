use std::path::PathBuf;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use penny_cost::import_pricebook_files;
use penny_proxy::{build_router, ProxyState};
use penny_store::SqliteStore;
use serde_json::{json, Value};
use sqlx::query_scalar;
use tower::ServiceExt;

fn prices_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../prices")
        .canonicalize()
        .expect("resolve prices dir")
}

async fn seed_pricebook(store: &SqliteStore) {
    let dir = prices_dir();
    import_pricebook_files(
        store,
        &[dir.join("anthropic.toml"), dir.join("openai.toml")],
    )
    .await
    .expect("import pricebook files");
}

#[tokio::test]
async fn chat_completion_persists_request_and_usage_end_to_end() {
    let store = SqliteStore::connect("sqlite::memory:")
        .await
        .expect("create in-memory store");
    seed_pricebook(&store).await;
    let app = build_router(ProxyState::mock_default().with_store(store.clone()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-6",
                        "messages": [{"role":"user","content":"integration test"}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let request_id = response
        .headers()
        .get("x-penny-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("request id header")
        .to_string();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let payload: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        payload["choices"][0]["message"]["content"],
        "Mock provider deterministic response."
    );

    let request_rows: i64 = query_scalar("SELECT COUNT(*) FROM requests WHERE id = ?1")
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("count requests");
    assert_eq!(request_rows, 1);

    let usage_rows: i64 = query_scalar("SELECT COUNT(*) FROM request_usage WHERE request_id = ?1")
        .bind(&request_id)
        .fetch_one(store.pool())
        .await
        .expect("count request_usage");
    assert_eq!(usage_rows, 1);
}
