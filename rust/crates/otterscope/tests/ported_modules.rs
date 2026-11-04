//! End-to-end checks over the real store: sample seeding, MCP tool queries and
//! the alert watcher's persisted firing state.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use otterscope::alerts::{Rule, Watcher, WebhookGate};
use otterscope::mcp::{Handler, PROTOCOL_VERSION};
use otterscope::store::{Filter, Store};
use otterscope::{VERSION, sample};

fn database_path(name: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("otterscope-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("test.db").to_string_lossy().into_owned()
}

fn allow_loopback() -> WebhookGate {
    Arc::new(|_| Box::pin(async { Ok(()) }))
}

#[tokio::test]
async fn sample_seeds_a_usable_database() {
    let store = Store::open(&database_path("sample")).expect("open store");
    sample::seed(&store, 6).await.expect("seed");

    let runs = store
        .list_runs(&Filter::default(), 100, 0)
        .expect("list runs");
    assert_eq!(runs.len(), 6);
    assert!(runs.windows(2).all(|pair| pair[0].start >= pair[1].start));

    let assertions = store.list_assertions("").expect("assertions");
    assert_eq!(assertions.len(), 2);

    let alerts = store.list_alerts("").expect("alerts");
    assert_eq!(alerts.len(), 3);
    assert_eq!(alerts.iter().filter(|alert| alert.firing).count(), 1);

    let stats = store.get_stats(&Filter::default()).expect("stats");
    assert_eq!(stats.runs, 6);
    assert_eq!(stats.assertion_rates.len(), 2);

    let results = store
        .results_for_run("default", &runs[0].id)
        .expect("results");
    assert_eq!(results.len(), 2);
}

fn mcp_headers(method: &str, name: Option<&str>) -> axum::http::HeaderMap {
    use axum::http::{HeaderValue, header::HeaderName};

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        HeaderName::from_static("mcp-protocol-version"),
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    headers.insert(
        HeaderName::from_static("mcp-method"),
        HeaderValue::from_str(method).expect("method header"),
    );
    if let Some(name) = name {
        headers.insert(
            HeaderName::from_static("mcp-name"),
            HeaderValue::from_str(name).expect("name header"),
        );
    }
    headers
}

fn mcp_body(method: &str, id: i64, params: serde_json::Value) -> Vec<u8> {
    let mut params = params.as_object().cloned().unwrap_or_default();
    params.insert(
        "_meta".into(),
        serde_json::json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "integration-test", "version": "1"},
        }),
    );
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request body")
}

#[tokio::test]
async fn mcp_tools_query_the_real_store() {
    let store = Arc::new(Store::open(&database_path("mcp")).expect("open store"));
    sample::seed(&*store, 4).await.expect("seed");
    let handler = Handler::new(store.clone(), VERSION);

    let body = mcp_body(
        "tools/call",
        1,
        serde_json::json!({"name": "list_runs", "arguments": {"limit": 10}}),
    );
    let reply = handler
        .handle(
            &axum::http::Method::POST,
            &mcp_headers("tools/call", Some("list_runs")),
            &body,
        )
        .await;
    assert_eq!(reply.status, axum::http::StatusCode::OK);
    let response: serde_json::Value = serde_json::from_slice(&reply.body).expect("json");
    assert_eq!(response["result"]["resultType"], "complete");
    assert_eq!(response["result"]["isError"], false);
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    let payload: serde_json::Value = serde_json::from_str(text).expect("payload");
    assert_eq!(payload["runs"].as_array().expect("runs").len(), 4);

    let body = mcp_body(
        "tools/call",
        2,
        serde_json::json!({"name": "get_stats", "arguments": {}}),
    );
    let reply = handler
        .handle(
            &axum::http::Method::POST,
            &mcp_headers("tools/call", Some("get_stats")),
            &body,
        )
        .await;
    let response: serde_json::Value = serde_json::from_slice(&reply.body).expect("json");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    let stats: serde_json::Value = serde_json::from_str(text).expect("stats");
    assert_eq!(stats["runs"], 4);
    assert!(stats["assertionRates"].is_array());
}

#[tokio::test]
async fn watcher_persists_firing_state_through_the_store() {
    let store = Arc::new(Store::open(&database_path("alerts")).expect("open store"));
    sample::seed(&*store, 3).await.expect("seed");

    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let app = Router::new().route(
        "/hook",
        post({
            let received = received.clone();
            move |body: String| {
                let received = received.clone();
                async move {
                    received.lock().expect("received").push(body);
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let rule = store
        .create_alert(Rule {
            project: "default".into(),
            name: "p95-tiny".into(),
            r#type: "p95_latency".into(),
            threshold: 1.0,
            window_secs: 8 * 86_400,
            webhook_url: format!("http://{addr}/hook"),
            enabled: true,
            ..Rule::default()
        })
        .expect("create alert");

    let watcher = Watcher::new(store.clone(), Duration::from_secs(3600))
        .expect("watcher")
        .with_webhook_gate(allow_loopback());
    watcher.evaluate_once().await;

    assert_eq!(received.lock().expect("received").len(), 1);
    let alerts = store.list_alerts("").expect("alerts");
    let fired = alerts
        .iter()
        .find(|alert| alert.id == rule.id)
        .expect("seeded rule");
    assert!(fired.firing);

    // Second cycle is steady state: no repeat notification.
    watcher.evaluate_once().await;
    assert_eq!(received.lock().expect("received").len(), 1);
    server.abort();
}
