//! End-to-end smoke tests for the axum server over the real SQLite store and
//! the real MCP handler — the wiring `serve` uses.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use chrono::{TimeZone, Utc};
use otterscope::evals::Assertion;
use otterscope::model::{Status, Step, StepKind};
use otterscope::server::{self, AppState, Hub, Options};
use otterscope::store::{Scope, Store};
use serde_json::Value;
use tower::ServiceExt;

fn temp_db() -> (PathBuf, Store) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "otterscope-server-api-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("test.db");
    let store = Store::open(path.to_str().expect("path")).expect("store");
    (dir, store)
}

fn app(store: Arc<Store>, options: Options) -> Router {
    server::router_with(AppState { store }, options, Hub::new())
}

fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "localhost:8080");
    if let Some(body) = body {
        builder = builder
            .header(header::CONTENT_LENGTH, body.len().to_string())
            .header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request")
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, headers, body.to_vec())
}

async fn send_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = send(app, request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn routes_work_against_the_real_store() {
    let (dir, store) = temp_db();
    let store = Arc::new(store);
    let app = app(store.clone(), Options::default());

    let (status, body) = send_json(&app, request("GET", "/healthz", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");

    let (status, body) = send_json(&app, request("GET", "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["runs"].as_array().map(Vec::len), Some(0));

    let (status, view) = send_json(
        &app,
        request(
            "POST",
            "/api/views",
            Some(r#"{"name":"prod","params":{"status":"error"}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(view["name"], "prod");
    let id = view["id"].as_i64().expect("id");

    let (status, conflict) = send_json(
        &app,
        request(
            "POST",
            "/api/views",
            Some(r#"{"name":"prod","params":{"status":"error"}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        conflict["error"]
            .as_str()
            .expect("error")
            .contains("already exists")
    );

    let (status, list) = send_json(&app, request("GET", "/api/views", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["views"].as_array().map(Vec::len), Some(1));

    let (status, _, _) = send(&app, request("DELETE", &format!("/api/views/{id}"), None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(&app, request("DELETE", &format!("/api/views/{id}"), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send_json(
        &app,
        request(
            "POST",
            "/api/assertions",
            Some(r#"{"name":"fast","type":"max_latency_ms","config":"20000"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, assertions) = send_json(&app, request("GET", "/api/assertions", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assertions["assertions"].as_array().map(Vec::len), Some(1));

    let (status, _) = send_json(
        &app,
        request("POST", "/api/assertions/evaluate?run=missing", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, metrics) = send(&app, request("GET", "/metrics", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&metrics).contains("otterscope_runs_total"));

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn mcp_is_mounted_and_negotiates_a_version() {
    let (dir, store) = temp_db();
    let store = Arc::new(store);
    let app = app(store.clone(), Options::default());

    let (status, body) = send_json(
        &app,
        request(
            "POST",
            "/mcp",
            Some(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}"#,
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn read_auth_uses_real_tokens() {
    let (dir, store) = temp_db();
    let store = Arc::new(store);
    let read = store.create_token("script", Scope::Read).expect("token");
    let app = app(
        store.clone(),
        Options {
            read_auth: true,
            ..Options::default()
        },
    );

    let (status, _, _) = send(&app, request("GET", "/api/runs", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut authorized = request("GET", "/api/runs", None);
    authorized.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", read.token).parse().expect("header"),
    );
    let (status, _, _) = send(&app, authorized).await;
    assert_eq!(status, StatusCode::OK);

    let mut mutation = request("POST", "/api/views", Some(r#"{"name":"v","params":{}}"#));
    mutation.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", read.token).parse().expect("header"),
    );
    let (status, _) = send_json(&app, mutation).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn evaluate_backfills_assertion_results_against_the_real_store() {
    let (dir, store) = temp_db();
    let start = Utc.timestamp_opt(1000, 0).single().expect("timestamp");
    store
        .upsert_steps(&[Step {
            id: "r1-root".into(),
            project: "default".into(),
            run_id: "r1".into(),
            parent_id: String::new(),
            kind: StepKind::Agent,
            name: "root".into(),
            service: "demo".into(),
            agent_name: "demo".into(),
            status: Status::Ok,
            start,
            end: Some(start + chrono::Duration::seconds(9)),
            error: String::new(),
            llm: None,
            tool: None,
        }])
        .expect("seed run");
    store
        .create_assertion(Assertion {
            name: "fast".into(),
            r#type: "max_latency_ms".into(),
            config: "20000".into(),
            enabled: true,
            ..Assertion::default()
        })
        .expect("assertion");

    let store = Arc::new(store);
    let app = app(store.clone(), Options::default());
    let (status, report) = send_json(
        &app,
        request("POST", "/api/assertions/evaluate?run=r1", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 1);

    let (status, detail) = send_json(&app, request("GET", "/api/runs/r1", None)).await;
    assert_eq!(status, StatusCode::OK);
    let results = detail["assertionResults"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], "fast");
    assert_eq!(results[0]["pass"], true);
    assert!(detail["run"]["costUsd"].is_null());

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
