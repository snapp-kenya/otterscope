//! End-to-end smoke tests for the axum server over the real SQLite store and
//! the real MCP handler — the wiring `serve` uses.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use chrono::{TimeZone, Utc};
use otterscope::evals::Assertion;
use otterscope::ingest::{IngestResult, Sink};
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
    server::router_with(AppState { store }, options, Hub::new(), Router::new())
}

/// A sink that persists straight to the real store, so an OTLP POST can be
/// observed through `/api/runs`.
struct StoreTestSink {
    store: Arc<Store>,
}

impl Sink for StoreTestSink {
    fn consume(&self, project: &str, raw: &[u8], steps: &[Step]) -> anyhow::Result<IngestResult> {
        let rejected = self.store.ingest_batch(project, raw, steps)?;
        Ok(IngestResult::with_rejections(rejected as i64))
    }
}

/// The exact wiring `serve` uses: one app, with the OTLP receiver mounted at
/// the root because the server's edge strips the base path once.
fn app_with_ingest(store: Arc<Store>, options: Options) -> Router {
    let ingest = otterscope::ingest::router(
        otterscope::ingest::Options {
            base_path: String::new(),
            ..Default::default()
        },
        Arc::new(StoreTestSink {
            store: store.clone(),
        }),
        Arc::new(|key: &str| key.is_empty().then(|| "default".to_string())),
    );
    server::router_with(AppState { store }, options, Hub::new(), ingest)
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

fn otlp_request(uri: &str) -> Request<Body> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/pydantic_ai_chat.json");
    let body = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "localhost:8080")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .body(Body::from(body))
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

async fn send_with_token(
    app: &Router,
    request: Request<Body>,
    token: &str,
) -> (StatusCode, Vec<u8>) {
    let mut request = request;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("header"),
    );
    let (status, _, body) = send(app, request).await;
    (status, body)
}

#[tokio::test]
async fn otlp_ingest_and_api_are_served_by_one_app() {
    // The same Router value answers an OTLP POST and the JSON API — there is no
    // second listener. Run it at the root and under a base path; the receiver
    // is passed an empty base path because the edge strips the prefix once.
    for base in ["", "/otterscope"] {
        let (dir, store) = temp_db();
        let store = Arc::new(store);
        let app = app_with_ingest(
            store.clone(),
            Options {
                base_path: base.to_string(),
                ..Options::default()
            },
        );

        let (status, _, body) = send(&app, otlp_request(&format!("{base}/v1/traces"))).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "base {base:?}: {}",
            String::from_utf8_lossy(&body)
        );

        let (status, runs) =
            send_json(&app, request("GET", &format!("{base}/api/runs"), None)).await;
        assert_eq!(status, StatusCode::OK, "base {base:?}");
        let runs = runs["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 1, "base {base:?}: the OTLP run is visible");
        let id = runs[0]["id"].as_str().expect("run id").to_string();

        let (status, detail) =
            send_json(&app, request("GET", &format!("{base}/api/runs/{id}"), None)).await;
        assert_eq!(status, StatusCode::OK, "base {base:?}");
        assert_eq!(
            detail["steps"].as_array().map(Vec::len),
            Some(3),
            "base {base:?}"
        );

        if !base.is_empty() {
            // Outside the prefix the endpoint is not served: no double mount.
            let (status, _, _) = send(&app, otlp_request("/v1/traces")).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "base {base:?}");
        }

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
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
async fn mcp_is_mounted_and_answers_discover() {
    let (dir, store) = temp_db();
    let store = Arc::new(store);
    let app = app(store.clone(), Options::default());

    let mut discover = request(
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"c","version":"1"}}}}"#,
        ),
    );
    discover.headers_mut().insert(
        "MCP-Protocol-Version",
        HeaderValue::from_static("2026-07-28"),
    );
    discover
        .headers_mut()
        .insert("Mcp-Method", HeaderValue::from_static("server/discover"));
    let (status, headers, raw) = send(&app, discover).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body: Value = serde_json::from_slice(&raw).expect("json");
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(
        body["result"]["supportedVersions"],
        serde_json::json!(["2026-07-28"])
    );
    assert_eq!(
        body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "otterscope"
    );

    // A legacy handshake is rejected, not negotiated downward.
    let mut legacy = request(
        "POST",
        "/mcp",
        Some(
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-06-18","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        ),
    );
    legacy.headers_mut().insert(
        "MCP-Protocol-Version",
        HeaderValue::from_static("2025-06-18"),
    );
    legacy
        .headers_mut()
        .insert("Mcp-Method", HeaderValue::from_static("initialize"));
    let (status, _, raw) = send(&app, legacy).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&raw).expect("json");
    assert_eq!(body["error"]["code"], -32022);

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn read_auth_uses_real_tokens() {
    let (dir, store) = temp_db();
    let store = Arc::new(store);
    let read = store.create_token("script", Scope::Read).expect("token");
    let admin = store
        .create_token("operator", Scope::Admin)
        .expect("admin token");
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

    // Publishing a run is a mutation: a read token cannot mint or revoke.
    let (status, _) = send_with_token(
        &app,
        request("POST", "/api/runs/r1/share", None),
        &read.token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, minted) = send_with_token(
        &app,
        request("POST", "/api/runs/r1/share", None),
        &admin.token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let minted: Value = serde_json::from_slice(&minted).expect("json");
    let share = minted["token"].as_str().expect("share token").to_string();

    let (status, shared) =
        send_json(&app, request("GET", &format!("/api/shared/{share}"), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["run"]["id"], "r1");

    let (status, _) = send_with_token(
        &app,
        request("DELETE", &format!("/api/shares/{share}"), None),
        &read.token,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send_json(&app, request("GET", &format!("/api/shared/{share}"), None)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_with_token(
        &app,
        request("DELETE", &format!("/api/shares/{share}"), None),
        &admin.token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send_json(&app, request("GET", &format!("/api/shared/{share}"), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

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
