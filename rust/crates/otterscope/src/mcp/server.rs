//! The MCP Streamable HTTP endpoint: JSON-RPC over `POST /mcp`
//! (port of `internal/mcp/server.go`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Map, Value, json};

use super::tools::{McpStore, run_tool, tool_defs};

/// The MCP revision this server prefers.
pub const LATEST_VERSION: &str = "2025-11-25";

/// Revisions this server will negotiate. The server echoes the client's
/// requested version when supported, else returns the latest (per the
/// lifecycle spec).
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

pub const SERVER_NAME: &str = "otterscope";

const INSTRUCTIONS: &str = "Query your agent's observability data: list_runs, get_run (with messages and tool i/o), get_stats, list_assertions.";

/// A transport-neutral HTTP response so the JSON-RPC behaviour is testable
/// without spinning up a server.
#[derive(Debug)]
pub struct HttpReply {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// Serves the MCP Streamable HTTP endpoint over the store.
pub struct Handler<S: McpStore> {
    store: Arc<S>,
    version: String,
}

impl<S: McpStore> Handler<S> {
    pub fn new(store: Arc<S>, version: impl Into<String>) -> Self {
        Self {
            store,
            version: version.into(),
        }
    }

    /// Handles a single JSON-RPC request per POST (Streamable HTTP,
    /// non-streaming: simple request/response tools return application/json).
    pub async fn handle(&self, method: &Method, headers: &HeaderMap, body: &[u8]) -> HttpReply {
        if method != Method::POST {
            return plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        // DNS-rebinding protection: reject cross-origin browser requests. MCP
        // clients (Claude Code, Inspector) send no Origin; browsers do.
        if let Some(origin) = header_str(headers, "origin")
            && !local_origin(origin)
        {
            return plain(StatusCode::FORBIDDEN, "forbidden origin");
        }

        let request = match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(map)) => map,
            // A JSON `null` decodes into a zero-value request in Go.
            Ok(Value::Null) => Map::new(),
            _ => return error_reply(None, -32700, "parse error"),
        };
        let rpc_method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = request.get("id").cloned();
        let params = request.get("params").cloned();

        // After init, MCP-Protocol-Version must be present and supported.
        // Absent is tolerated (spec default); present-but-unsupported is a 400.
        if rpc_method != "initialize"
            && let Some(version) = header_str(headers, "mcp-protocol-version")
            && !SUPPORTED_VERSIONS.contains(&version)
        {
            return plain(StatusCode::BAD_REQUEST, "unsupported MCP-Protocol-Version");
        }

        // Notifications (no id) get 202 Accepted with no body.
        let Some(id) = id else {
            return HttpReply {
                status: StatusCode::ACCEPTED,
                content_type: "text/plain; charset=utf-8",
                body: Vec::new(),
            };
        };

        match rpc_method {
            "initialize" => result_reply(
                &id,
                &json!({
                    "protocolVersion": negotiate_version(params.as_ref()),
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "title": "Otterscope",
                        "version": self.version,
                    },
                    "instructions": INSTRUCTIONS,
                }),
            ),
            "ping" => result_reply(&id, &json!({})),
            "tools/list" => result_reply(&id, &json!({"tools": tool_list()})),
            "tools/call" => self.call_tool(&id, params.as_ref()).await,
            _ => error_reply(
                Some(&id),
                -32601,
                &format!("method not found: {rpc_method}"),
            ),
        }
    }

    async fn call_tool(&self, id: &Value, params: Option<&Value>) -> HttpReply {
        let (name, arguments) = match params {
            None | Some(Value::Null) => (String::new(), Map::new()),
            Some(Value::Object(params)) => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = match params.get("arguments") {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(args)) => args.clone(),
                    Some(_) => return error_reply(Some(id), -32602, "invalid params"),
                };
                (name, arguments)
            }
            Some(_) => return error_reply(Some(id), -32602, "invalid params"),
        };
        if !tool_defs().iter().any(|tool| tool.name == name) {
            return error_reply(Some(id), -32602, &format!("unknown tool: {name}"));
        }
        // A tool error is reported in-band (isError: true), not as a JSON-RPC
        // error, so the model can read and react to it.
        match run_tool(&*self.store, &name, &arguments).await {
            Ok(text) => result_reply(id, &tool_result(&text, false)),
            Err(err) => result_reply(id, &tool_result(&format!("{err:#}"), true)),
        }
    }

    /// Runs the handler behind an `axum` route.
    pub async fn serve(&self, request: axum::extract::Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
            Ok(body) => body,
            Err(_) => {
                return plain_response(StatusCode::BAD_REQUEST, "invalid body");
            }
        };
        let reply = self.handle(&parts.method, &parts.headers, &body).await;
        let mut response = Response::new(Body::from(reply.body));
        *response.status_mut() = reply.status;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(reply.content_type),
        );
        response
    }
}

impl<S: McpStore + 'static> Handler<S> {
    /// A router serving `POST /mcp`, for servers that prefer not to wire the
    /// handler by hand.
    pub fn into_router(self) -> axum::Router {
        let handler = Arc::new(self);
        axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |request: axum::extract::Request| {
                let handler = handler.clone();
                async move { handler.serve(request).await }
            }),
        )
    }
}

/// Echoes the client's requested protocol version when supported, else the
/// server's latest (lifecycle spec §negotiation).
fn negotiate_version(params: Option<&Value>) -> &'static str {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str);
    if let Some(requested) = requested
        && let Some(supported) = SUPPORTED_VERSIONS.iter().find(|v| **v == requested)
    {
        return supported;
    }
    LATEST_VERSION
}

fn tool_list() -> Vec<Value> {
    tool_defs()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": tool.input_schema,
                // Every Otterscope tool is read-only.
                "annotations": {"readOnlyHint": true},
            })
        })
        .collect()
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

/// Reports whether an Origin header points at loopback.
fn local_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host == "127.0.0.1" || host == "localhost" || host == "::1" || host.starts_with("127.")
}

fn plain(status: StatusCode, message: &str) -> HttpReply {
    HttpReply {
        status,
        content_type: "text/plain; charset=utf-8",
        body: format!("{message}\n").into_bytes(),
    }
}

fn plain_response(status: StatusCode, message: &str) -> Response {
    let reply = plain(status, message);
    let mut response = Response::new(Body::from(reply.body));
    *response.status_mut() = reply.status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(reply.content_type),
    );
    response
}

fn json_reply(status: StatusCode, value: &Value) -> HttpReply {
    let mut body = serde_json::to_vec(value).unwrap_or_default();
    body.push(b'\n');
    HttpReply {
        status,
        content_type: "application/json",
        body,
    }
}

fn result_reply(id: &Value, result: &Value) -> HttpReply {
    json_reply(
        StatusCode::OK,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
}

fn error_reply(id: Option<&Value>, code: i32, message: &str) -> HttpReply {
    let mut response = json!({"jsonrpc": "2.0", "error": {"code": code, "message": message}});
    if let Some(id) = id {
        response["id"] = id.clone();
    }
    json_reply(StatusCode::OK, &response)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::http::{HeaderName, Method};
    use chrono::DateTime;

    use super::*;
    use crate::evals::Assertion;
    use crate::model::{LlmCall, Message, Run, Status, Step, StepKind};
    use crate::store::{Filter, Stats};

    #[derive(Default)]
    struct FakeStore {
        runs: Mutex<Vec<(Run, Vec<Step>)>>,
        assertions: Mutex<Vec<Assertion>>,
    }

    impl FakeStore {
        fn seed(&self, id: &str, start_sec: i64) {
            let start = DateTime::from_timestamp(start_sec, 0).expect("start");
            let end = start + chrono::Duration::seconds(5);
            let run = Run {
                id: id.to_string(),
                project: "default".into(),
                service: "svc".into(),
                agent_name: "demo".into(),
                status: Status::Ok,
                start,
                end: Some(end),
                input_tokens: 100,
                output_tokens: 20,
                llm_calls: 1,
                tool_calls: 0,
                models: "claude-sonnet-5".into(),
                prompts: String::new(),
                cost_usd: Some(0.001),
                cost_partial: false,
                error: String::new(),
            };
            let steps = vec![
                Step {
                    id: format!("{id}-root"),
                    project: "default".into(),
                    run_id: id.to_string(),
                    parent_id: String::new(),
                    kind: StepKind::Agent,
                    name: "invoke_agent demo".into(),
                    service: "svc".into(),
                    agent_name: "demo".into(),
                    status: Status::Ok,
                    start,
                    end: Some(end),
                    error: String::new(),
                    llm: None,
                    tool: None,
                },
                Step {
                    id: format!("{id}-llm"),
                    project: "default".into(),
                    run_id: id.to_string(),
                    parent_id: format!("{id}-root"),
                    kind: StepKind::Llm,
                    name: "chat m1".into(),
                    service: "svc".into(),
                    agent_name: "demo".into(),
                    status: Status::Ok,
                    start: start + chrono::Duration::seconds(1),
                    end: Some(start + chrono::Duration::seconds(3)),
                    error: String::new(),
                    llm: Some(LlmCall {
                        request_model: "claude-sonnet-5".into(),
                        input_tokens: 100,
                        output_tokens: 20,
                        input_messages: vec![Message {
                            role: "user".into(),
                            content: "hi".into(),
                        }],
                        output_messages: vec![Message {
                            role: "assistant".into(),
                            content: "hello".into(),
                        }],
                        ..LlmCall::default()
                    }),
                    tool: None,
                },
            ];
            self.runs.lock().expect("runs").push((run, steps));
        }
    }

    impl McpStore for FakeStore {
        async fn list_runs(
            &self,
            filter: &Filter,
            limit: i64,
            offset: i64,
        ) -> anyhow::Result<Vec<Run>> {
            let mut runs: Vec<Run> = self
                .runs
                .lock()
                .expect("runs")
                .iter()
                .map(|(run, _)| run.clone())
                .filter(|run| {
                    (filter.project.is_empty() || run.project == filter.project)
                        && (filter.status.is_empty() || run.status.as_str() == filter.status)
                        && (filter.service.is_empty() || run.service == filter.service)
                        && (filter.model.is_empty() || run.models.contains(&filter.model))
                })
                .collect();
            runs.sort_by_key(|run| std::cmp::Reverse(run.start));
            Ok(runs
                .into_iter()
                .skip(offset.max(0) as usize)
                .take(limit.max(0) as usize)
                .collect())
        }

        async fn get_run(&self, id: &str) -> anyhow::Result<Option<(Run, Vec<Step>)>> {
            Ok(self
                .runs
                .lock()
                .expect("runs")
                .iter()
                .find(|(run, _)| run.id == id)
                .cloned())
        }

        async fn get_stats(&self, filter: &Filter) -> anyhow::Result<Stats> {
            let runs = self.list_runs(filter, 1_000_000, 0).await?;
            let errors = runs.iter().filter(|r| r.status == Status::Error).count() as i64;
            let cost: Option<f64> = runs.iter().filter_map(|r| r.cost_usd).reduce(|a, b| a + b);
            Ok(Stats {
                runs: runs.len() as i64,
                errors,
                total_cost_usd: cost,
                input_tokens: runs.iter().map(|r| r.input_tokens).sum(),
                output_tokens: runs.iter().map(|r| r.output_tokens).sum(),
                ..Stats::default()
            })
        }

        async fn list_assertions(&self, project: &str) -> anyhow::Result<Vec<Assertion>> {
            Ok(self
                .assertions
                .lock()
                .expect("assertions")
                .iter()
                .filter(|a| project.is_empty() || a.project == project)
                .cloned()
                .collect())
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        headers
    }

    async fn do_request(
        handler: &Handler<FakeStore>,
        body: &str,
        extra: &[(&str, &str)],
    ) -> HttpReply {
        handler
            .handle(&Method::POST, &headers(extra), body.as_bytes())
            .await
    }

    fn decode(reply: &HttpReply) -> Value {
        serde_json::from_slice(&reply.body).expect("json-rpc response")
    }

    #[tokio::test]
    async fn initialize_echoes_supported_version() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}"#,
            &[],
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.content_type, "application/json");
        let response = decode(&reply);
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert_eq!(response["result"]["serverInfo"]["name"], "otterscope");

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01","capabilities":{}}}"#,
            &[],
        )
        .await;
        assert_eq!(decode(&reply)["result"]["protocolVersion"], LATEST_VERSION);
    }

    #[tokio::test]
    async fn notification_returns_202() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &[],
        )
        .await;
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert!(reply.body.is_empty());
    }

    #[tokio::test]
    async fn tools_list_and_call() {
        let store = Arc::new(FakeStore::default());
        store.seed("r1", 1000);
        let handler = Handler::new(store, "test");

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
            &[("MCP-Protocol-Version", "2025-11-25")],
        )
        .await;
        let response = decode(&reply);
        let tools = response["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() >= 3);
        assert!(tools[0]["inputSchema"].is_object());
        assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_run","arguments":{"id":"r1"}}}"#,
            &[("MCP-Protocol-Version", "2025-11-25")],
        )
        .await;
        let call = decode(&reply);
        assert_eq!(call["result"]["isError"], false);
        let text = call["result"]["content"][0]["text"]
            .as_str()
            .expect("content text");
        let payload: Value = serde_json::from_str(text).expect("tool payload");
        assert_eq!(payload["run"]["id"], "r1");
        assert_eq!(payload["steps"].as_array().expect("steps").len(), 2);
        let llm = payload["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .find(|step| step["kind"] == "llm")
            .expect("llm step");
        assert!(llm["llm"]["inputMessages"].is_array());
    }

    #[tokio::test]
    async fn tool_errors_are_in_band() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_run","arguments":{"id":"missing"}}}"#,
            &[],
        )
        .await;
        let response = decode(&reply);
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            &[],
        )
        .await;
        let response = decode(&reply);
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["message"], "unknown tool: nope");
    }

    #[tokio::test]
    async fn rejects_bad_origin_and_version() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;

        let reply = do_request(&handler, body, &[("Origin", "https://evil.example.com")]).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);

        let reply = do_request(&handler, body, &[("MCP-Protocol-Version", "1999-01-01")]).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);

        let reply = do_request(&handler, body, &[("Origin", "http://localhost:8317")]).await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn non_post_and_malformed_requests() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let reply = handler.handle(&Method::GET, &HeaderMap::new(), b"").await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);

        let reply = do_request(&handler, "not json", &[]).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(decode(&reply)["error"]["code"], -32700);

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":9,"method":"does/not/exist"}"#,
            &[],
        )
        .await;
        assert_eq!(decode(&reply)["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn list_runs_and_stats_tools() {
        let store = Arc::new(FakeStore::default());
        store.seed("old", 1000);
        store.seed("new", 2000);
        let handler = Handler::new(store, "test");

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"list_runs","arguments":{"limit":10}}}"#,
            &[],
        )
        .await;
        let text = decode(&reply)["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        let payload: Value = serde_json::from_str(&text).expect("payload");
        let runs = payload["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["id"], "new");

        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_stats","arguments":{}}}"#,
            &[],
        )
        .await;
        let text = decode(&reply)["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        let stats: Value = serde_json::from_str(&text).expect("stats");
        assert_eq!(stats["runs"], 2);
        assert!(stats.get("totalCostUsd").is_some());
        assert!(stats.get("avgDurationMs").is_some());
        assert!(stats["assertionRates"].is_null());
    }

    #[tokio::test]
    async fn get_run_requires_an_id() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let reply = do_request(
            &handler,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"get_run","arguments":{}}}"#,
            &[],
        )
        .await;
        let response = decode(&reply);
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("id is required"))
        );
    }

    #[tokio::test]
    async fn serves_an_http_request() {
        let store = Arc::new(FakeStore::default());
        store.seed("r1", 1000);
        let handler = Handler::new(store, "test");
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
            .expect("request");
        let response = handler.serve(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body).expect("json");
        assert!(value["result"].is_object());
    }
}
