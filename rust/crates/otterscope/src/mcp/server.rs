//! The MCP Streamable HTTP endpoint: JSON-RPC over `POST /mcp`.
//!
//! Implements only protocol revision 2026-07-28, the stateless revision: there
//! is no `initialize` handshake, no session header and no resumable SSE. Every
//! request declares its version, capabilities and identity in `_meta`, and the
//! HTTP binding mirrors the version and method into headers that must agree
//! with the body.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use serde_json::{Map, Value, json};

use super::tools::{McpStore, run_tool, tool_defs};

/// The only protocol revision this server implements.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// Revisions advertised by `server/discover` and accepted for requests.
pub const SUPPORTED_VERSIONS: [&str; 1] = [PROTOCOL_VERSION];

pub const SERVER_NAME: &str = "otterscope";

/// `tools/list` freshness hint: the tool set changes only with a deployment.
const TOOLS_TTL_MS: i64 = 300_000;

/// `server/discover` freshness hint (the example in the spec uses one hour).
const DISCOVER_TTL_MS: i64 = 3_600_000;

/// Tool definitions do not vary by caller, so shared caches may keep them.
const CACHE_SCOPE: &str = "public";

const INSTRUCTIONS: &str = "Query your agent's observability data: list_runs, get_run (with messages and tool i/o), get_stats, list_assertions.";

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const HEADER_METHOD: &str = "mcp-method";
const HEADER_NAME: &str = "mcp-name";

const BASE64_PREFIX: &str = "=?base64?";
const BASE64_SUFFIX: &str = "?=";

/// A transport-neutral HTTP response so the JSON-RPC behaviour is testable
/// without spinning up a server.
#[derive(Debug)]
pub struct HttpReply {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// Serves the stateless MCP Streamable HTTP endpoint over the store.
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

    /// Handles a single JSON-RPC message per POST.
    ///
    /// Streamable HTTP is non-streaming here: tools return application/json and
    /// the server never opens an SSE stream, so there is nothing to resume and
    /// `Last-Event-ID` is ignored.
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
            _ => return json_error(StatusCode::OK, None, -32700, "parse error", None),
        };
        let Some(rpc_method) = request.get("method").and_then(Value::as_str) else {
            return json_error(
                StatusCode::OK,
                None,
                -32600,
                "invalid request: method must be a string",
                None,
            );
        };
        let rpc_method = rpc_method.to_string();
        let id = match request.get("id") {
            None => None,
            Some(value @ (Value::String(_) | Value::Number(_))) => Some(value.clone()),
            Some(_) => {
                return json_error(
                    StatusCode::OK,
                    None,
                    -32600,
                    "invalid request: id must be a string or integer",
                    None,
                );
            }
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        // The standard headers are required on every POST. `Mcp-Session-Id`
        // and `Last-Event-ID` are deliberately ignored: this revision has no
        // sessions and no resumable streams.
        let Some(header_version) = header_str(headers, HEADER_PROTOCOL_VERSION) else {
            return header_mismatch(id.as_ref(), "missing required header MCP-Protocol-Version");
        };
        let Some(header_method) = header_str(headers, HEADER_METHOD) else {
            return header_mismatch(id.as_ref(), "missing required header Mcp-Method");
        };
        if header_method != rpc_method.as_str() {
            return header_mismatch(
                id.as_ref(),
                format!(
                    "Mcp-Method header value {header_method:?} does not match body value {rpc_method:?}"
                ),
            );
        }

        // Notifications carry no id and get 202 with no body (transport rule).
        let Some(id) = id else {
            return HttpReply {
                status: StatusCode::ACCEPTED,
                content_type: "text/plain; charset=utf-8",
                body: Vec::new(),
            };
        };

        if let Err(err) = validate_request(headers, &params, &rpc_method, header_version) {
            return match err {
                ValidationError::Header(message) => header_mismatch(Some(&id), &message),
                ValidationError::InvalidParams(message) => json_error(
                    StatusCode::BAD_REQUEST,
                    Some(&id),
                    -32602,
                    &format!("Invalid params: {message}"),
                    None,
                ),
                ValidationError::UnsupportedVersion(requested) => {
                    unsupported_version(&id, &requested)
                }
            };
        }

        match rpc_method.as_str() {
            "server/discover" => result_reply(&id, &self.discover_result()),
            "tools/list" => result_reply(&id, &self.tools_result()),
            "tools/call" => self.call_tool(&id, &params).await,
            other => json_error(
                StatusCode::NOT_FOUND,
                Some(&id),
                -32601,
                &format!("Method not found: {other}"),
                None,
            ),
        }
    }

    async fn call_tool(&self, id: &Value, params: &Value) -> HttpReply {
        let params = params.as_object();
        let name = params
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let arguments = match params.and_then(|params| params.get("arguments")) {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => {
                return json_error(StatusCode::OK, Some(id), -32602, "Invalid params", None);
            }
        };
        if !tool_defs().iter().any(|tool| tool.name == name) {
            return json_error(
                StatusCode::OK,
                Some(id),
                -32602,
                &format!("Unknown tool: {name}"),
                None,
            );
        }
        // A tool error is reported in-band (isError: true), not as a JSON-RPC
        // error, so the model can read and react to it.
        match run_tool(&*self.store, &name, &arguments).await {
            Ok(text) => result_reply(id, &self.complete_result(tool_result(&text, false))),
            Err(err) => result_reply(
                id,
                &self.complete_result(tool_result(&format!("{err:#}"), true)),
            ),
        }
    }

    fn discover_result(&self) -> Value {
        self.complete_result(json!({
            "supportedVersions": SUPPORTED_VERSIONS,
            "capabilities": {"tools": {"listChanged": false}},
            "instructions": INSTRUCTIONS,
            "ttlMs": DISCOVER_TTL_MS,
            "cacheScope": CACHE_SCOPE,
        }))
    }

    fn tools_result(&self) -> Value {
        let mut defs = tool_defs();
        // Deterministic order so clients can cache the list and reuse it in
        // prompt prefixes.
        defs.sort_by_key(|tool| tool.name);
        let tools: Vec<Value> = defs
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
            .collect();
        self.complete_result(json!({
            "tools": tools,
            "ttlMs": TOOLS_TTL_MS,
            "cacheScope": CACHE_SCOPE,
        }))
    }

    /// Adds the required `resultType` and identifies the server in every
    /// result's `_meta`.
    fn complete_result(&self, mut result: Value) -> Value {
        result["resultType"] = json!("complete");
        let mut meta = Map::new();
        meta.insert(META_SERVER_INFO.to_string(), self.server_info());
        result["_meta"] = Value::Object(meta);
        result
    }

    fn server_info(&self) -> Value {
        json!({
            "name": SERVER_NAME,
            "title": "Otterscope",
            "version": self.version,
        })
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

enum ValidationError {
    Header(String),
    InvalidParams(String),
    UnsupportedVersion(String),
}

/// Validates the per-request `_meta` fields and the mirrored headers against
/// the request body.
fn validate_request(
    headers: &HeaderMap,
    params: &Value,
    rpc_method: &str,
    header_version: &str,
) -> Result<(), ValidationError> {
    let Some(meta) = params.get("_meta").and_then(Value::as_object) else {
        return Err(ValidationError::InvalidParams(
            "missing required _meta".to_string(),
        ));
    };
    let Some(meta_version) = meta.get(META_PROTOCOL_VERSION).and_then(Value::as_str) else {
        return Err(ValidationError::InvalidParams(format!(
            "missing required _meta.{META_PROTOCOL_VERSION}"
        )));
    };
    if !meta
        .get(META_CLIENT_CAPABILITIES)
        .is_some_and(Value::is_object)
    {
        return Err(ValidationError::InvalidParams(format!(
            "missing required _meta.{META_CLIENT_CAPABILITIES}"
        )));
    }
    if meta_version != header_version {
        return Err(ValidationError::Header(format!(
            "MCP-Protocol-Version header value {header_version:?} does not match body value {meta_version:?}"
        )));
    }
    if !SUPPORTED_VERSIONS.contains(&meta_version) {
        return Err(ValidationError::UnsupportedVersion(
            meta_version.to_string(),
        ));
    }
    if requires_name(rpc_method) {
        let Some(header_name) = header_str(headers, HEADER_NAME) else {
            return Err(ValidationError::Header(
                "missing required header Mcp-Name".to_string(),
            ));
        };
        let header_name = decode_header_value(header_name).map_err(|reason| {
            ValidationError::Header(format!("Mcp-Name header is malformed: {reason}"))
        })?;
        let body_name = params.get("name").and_then(Value::as_str);
        if body_name != Some(header_name.as_str()) {
            return Err(ValidationError::Header(format!(
                "Mcp-Name header value {header_name:?} does not match body value {body_name:?}"
            )));
        }
    }
    Ok(())
}

/// Methods whose `params.name` is mirrored into `Mcp-Name`. Only `tools/call`
/// is implemented, but the transport rule covers the other two.
fn requires_name(rpc_method: &str) -> bool {
    matches!(rpc_method, "tools/call" | "resources/read" | "prompts/get")
}

/// Decodes the `=?base64?...?=` sentinel the transport defines for header
/// values that are not safe plain ASCII.
fn decode_header_value(value: &str) -> Result<String, &'static str> {
    let Some(encoded) = value
        .strip_prefix(BASE64_PREFIX)
        .and_then(|rest| rest.strip_suffix(BASE64_SUFFIX))
    else {
        return Ok(value.to_string());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "invalid base64")?;
    String::from_utf8(bytes).map_err(|_| "invalid UTF-8")
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

fn json_error(
    status: StatusCode,
    id: Option<&Value>,
    code: i32,
    message: &str,
    data: Option<Value>,
) -> HttpReply {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data {
        error["data"] = data;
    }
    let mut response = json!({"jsonrpc": "2.0", "error": error});
    if let Some(id) = id {
        response["id"] = id.clone();
    }
    json_reply(status, &response)
}

fn header_mismatch(id: Option<&Value>, detail: impl AsRef<str>) -> HttpReply {
    json_error(
        StatusCode::BAD_REQUEST,
        id,
        -32020,
        &format!("Header mismatch: {}", detail.as_ref()),
        None,
    )
}

fn unsupported_version(id: &Value, requested: &str) -> HttpReply {
    json_error(
        StatusCode::BAD_REQUEST,
        Some(id),
        -32022,
        "Unsupported protocol version",
        Some(json!({
            "supported": SUPPORTED_VERSIONS,
            "requested": requested,
        })),
    )
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

    fn standard_headers(method: &str) -> HeaderMap {
        headers(&[
            ("MCP-Protocol-Version", PROTOCOL_VERSION),
            ("Mcp-Method", method),
        ])
    }

    fn tool_headers(name: &str) -> HeaderMap {
        let mut headers = standard_headers("tools/call");
        headers.insert(
            HeaderName::from_static(HEADER_NAME),
            HeaderValue::from_str(name).expect("tool name"),
        );
        headers
    }

    fn meta(version: &str) -> Value {
        let mut meta = Map::new();
        meta.insert(META_PROTOCOL_VERSION.to_string(), json!(version));
        meta.insert(META_CLIENT_CAPABILITIES.to_string(), json!({}));
        meta.insert(
            "io.modelcontextprotocol/clientInfo".to_string(),
            json!({"name": "test-client", "version": "1.0.0"}),
        );
        Value::Object(meta)
    }

    fn request(method: &str, id: Value, params: Map<String, Value>) -> String {
        request_with_meta(method, id, params, meta(PROTOCOL_VERSION))
    }

    fn request_with_meta(
        method: &str,
        id: Value,
        mut params: Map<String, Value>,
        meta: Value,
    ) -> String {
        params.insert("_meta".to_string(), meta);
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    fn tool_call_body(name: &str, arguments: Value) -> String {
        let mut params = Map::new();
        params.insert("name".to_string(), json!(name));
        params.insert("arguments".to_string(), arguments);
        request("tools/call", json!(1), params)
    }

    async fn post(handler: &Handler<FakeStore>, body: &str, headers: &HeaderMap) -> HttpReply {
        handler
            .handle(&Method::POST, headers, body.as_bytes())
            .await
    }

    fn decode(reply: &HttpReply) -> Value {
        serde_json::from_slice(&reply.body).expect("json-rpc response")
    }

    #[tokio::test]
    async fn discover_advertises_versions_capabilities_and_identity() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = request("server/discover", json!("discover-1"), Map::new());
        let reply = post(&handler, &body, &standard_headers("server/discover")).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.content_type, "application/json");

        let response = decode(&reply);
        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["supportedVersions"], json!([PROTOCOL_VERSION]));
        assert!(result["capabilities"]["tools"].is_object());
        assert!(
            result["instructions"]
                .as_str()
                .is_some_and(|instructions| !instructions.is_empty())
        );
        assert!(result["ttlMs"].is_number());
        assert_eq!(result["cacheScope"], "public");
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], SERVER_NAME);
        assert_eq!(result["_meta"][META_SERVER_INFO]["version"], "test");
    }

    #[tokio::test]
    async fn rejects_unsupported_protocol_version() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let mut meta = Map::new();
        meta.insert(META_PROTOCOL_VERSION.to_string(), json!("2025-11-25"));
        meta.insert(META_CLIENT_CAPABILITIES.to_string(), json!({}));
        // The header and body agree, so this is a version rejection and not a
        // header mismatch.
        let headers = headers(&[
            ("MCP-Protocol-Version", "2025-11-25"),
            ("Mcp-Method", "tools/list"),
        ]);
        let body = request_with_meta("tools/list", json!(1), Map::new(), Value::Object(meta));
        let reply = post(&handler, &body, &headers).await;

        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        let response = decode(&reply);
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(
            response["error"]["data"]["supported"],
            json!([PROTOCOL_VERSION])
        );
        assert_eq!(response["error"]["data"]["requested"], "2025-11-25");
    }

    #[tokio::test]
    async fn rejects_header_body_version_mismatch() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        for (header_version, body_version) in [
            (PROTOCOL_VERSION, "2025-11-25"),
            ("2025-11-25", PROTOCOL_VERSION),
        ] {
            let headers = headers(&[
                ("MCP-Protocol-Version", header_version),
                ("Mcp-Method", "tools/list"),
            ]);
            let body = request_with_meta("tools/list", json!(1), Map::new(), meta(body_version));
            let reply = post(&handler, &body, &headers).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST);
            let response = decode(&reply);
            assert_eq!(response["error"]["code"], -32020);
            assert!(
                response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("Header mismatch:"))
            );
        }
    }

    #[tokio::test]
    async fn requires_standard_headers() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = request("tools/list", json!(1), Map::new());

        for missing in [HEADER_PROTOCOL_VERSION, HEADER_METHOD] {
            let mut request_headers = standard_headers("tools/list");
            request_headers.remove(missing);
            let reply = post(&handler, &body, &request_headers).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "missing {missing}");
            assert_eq!(decode(&reply)["error"]["code"], -32020);
        }

        let wrong_method = headers(&[
            ("MCP-Protocol-Version", PROTOCOL_VERSION),
            ("Mcp-Method", "tools/call"),
        ]);
        let reply = post(&handler, &body, &wrong_method).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32020);

        let call = tool_call_body("get_run", json!({"id": "r1"}));
        let no_name = standard_headers("tools/call");
        let reply = post(&handler, &call, &no_name).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32020);

        let reply = post(&handler, &call, &tool_headers("list_runs")).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert!(
            decode(&reply)["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("Mcp-Name"))
        );
    }

    #[tokio::test]
    async fn decodes_base64_mcp_name() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let encoded = base64::engine::general_purpose::STANDARD.encode("get_run");
        let mut headers = standard_headers("tools/call");
        headers.insert(
            HeaderName::from_static(HEADER_NAME),
            HeaderValue::from_str(&format!("{BASE64_PREFIX}{encoded}{BASE64_SUFFIX}"))
                .expect("encoded header"),
        );
        // No id argument: the tool fails in-band, which proves the header was
        // accepted and decoded.
        let body = tool_call_body("get_run", json!({}));
        let reply = post(&handler, &body, &headers).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(decode(&reply)["result"]["isError"], true);

        let mut malformed = standard_headers("tools/call");
        malformed.insert(
            HeaderName::from_static(HEADER_NAME),
            HeaderValue::from_static("=?base64?!!!?="),
        );
        let reply = post(&handler, &body, &malformed).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32020);
    }

    #[tokio::test]
    async fn requires_request_meta() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let headers = standard_headers("tools/list");

        let no_meta = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
        })
        .to_string();
        let reply = post(&handler, &no_meta, &headers).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32602);

        let mut missing_version = Map::new();
        missing_version.insert(META_CLIENT_CAPABILITIES.to_string(), json!({}));
        let body = request_with_meta(
            "tools/list",
            json!(1),
            Map::new(),
            Value::Object(missing_version),
        );
        let reply = post(&handler, &body, &headers).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32602);

        let mut missing_capabilities = Map::new();
        missing_capabilities.insert(META_PROTOCOL_VERSION.to_string(), json!(PROTOCOL_VERSION));
        let body = request_with_meta(
            "tools/list",
            json!(1),
            Map::new(),
            Value::Object(missing_capabilities),
        );
        let reply = post(&handler, &body, &headers).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(decode(&reply)["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn tools_list_is_cacheable_and_deterministic() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = request("tools/list", json!(2), Map::new());
        let headers = standard_headers("tools/list");

        let first = post(&handler, &body, &headers).await;
        let second = post(&handler, &body, &headers).await;
        assert_eq!(first.status, StatusCode::OK);
        let response = decode(&first);
        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert!(result["ttlMs"].is_number());
        assert_eq!(result["cacheScope"], "public");

        let names: Vec<&str> = result["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect();
        assert_eq!(
            names,
            vec!["get_run", "get_stats", "list_assertions", "list_runs"]
        );
        assert_eq!(result["tools"], decode(&second)["result"]["tools"]);

        let first_tool = &result["tools"][0];
        assert!(first_tool["inputSchema"].is_object());
        assert_eq!(first_tool["annotations"]["readOnlyHint"], true);
    }

    #[tokio::test]
    async fn tools_call_returns_complete_result_and_server_info() {
        let store = Arc::new(FakeStore::default());
        store.seed("r1", 1000);
        let handler = Handler::new(store, "test");
        let body = tool_call_body("get_run", json!({"id": "r1"}));
        let reply = post(&handler, &body, &tool_headers("get_run")).await;

        assert_eq!(reply.status, StatusCode::OK);
        let response = decode(&reply);
        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["isError"], false);
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], SERVER_NAME);

        let text = result["content"][0]["text"].as_str().expect("content text");
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

        let body = tool_call_body("get_run", json!({"id": "missing"}));
        let reply = post(&handler, &body, &tool_headers("get_run")).await;
        let response = decode(&reply);
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["resultType"], "complete");
        assert_eq!(response["result"]["isError"], true);

        let body = tool_call_body("nope", json!({}));
        let reply = post(&handler, &body, &tool_headers("nope")).await;
        let response = decode(&reply);
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["message"], "Unknown tool: nope");
    }

    #[tokio::test]
    async fn get_run_requires_an_id() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = tool_call_body("get_run", json!({}));
        let reply = post(&handler, &body, &tool_headers("get_run")).await;
        let response = decode(&reply);
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("id is required"))
        );
    }

    #[tokio::test]
    async fn list_runs_and_stats_tools() {
        let store = Arc::new(FakeStore::default());
        store.seed("old", 1000);
        store.seed("new", 2000);
        let handler = Handler::new(store, "test");

        let body = tool_call_body("list_runs", json!({"limit": 10}));
        let reply = post(&handler, &body, &tool_headers("list_runs")).await;
        let text = decode(&reply)["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        let payload: Value = serde_json::from_str(&text).expect("payload");
        let runs = payload["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["id"], "new");

        let body = tool_call_body("get_stats", json!({}));
        let reply = post(&handler, &body, &tool_headers("get_stats")).await;
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
    async fn removed_methods_are_not_found() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        for method in [
            "initialize",
            "ping",
            "logging/setLevel",
            "notifications/initialized",
        ] {
            let body = request(method, json!(1), Map::new());
            let reply = post(&handler, &body, &standard_headers(method)).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{method}");
            let response = decode(&reply);
            assert_eq!(response["error"]["code"], -32601, "{method}");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("Method not found:"))
            );
        }
    }

    #[tokio::test]
    async fn notifications_return_202() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": 1},
        })
        .to_string();
        let reply = post(
            &handler,
            &body,
            &standard_headers("notifications/cancelled"),
        )
        .await;
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert!(reply.body.is_empty());
    }

    #[tokio::test]
    async fn only_post_is_accepted() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        for method in [Method::GET, Method::DELETE] {
            let reply = handler.handle(&method, &HeaderMap::new(), b"").await;
            assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
        }
    }

    #[tokio::test]
    async fn rejects_bad_origin() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let body = request("tools/list", json!(1), Map::new());
        let mut headers = standard_headers("tools/list");
        headers.insert(
            HeaderName::from_static("origin"),
            HeaderValue::from_static("https://evil.example.com"),
        );
        let reply = post(&handler, &body, &headers).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);

        headers.insert(
            HeaderName::from_static("origin"),
            HeaderValue::from_static("http://localhost:8317"),
        );
        let reply = post(&handler, &body, &headers).await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn parse_and_invalid_request_errors() {
        let handler = Handler::new(Arc::new(FakeStore::default()), "test");
        let headers = standard_headers("tools/list");

        let reply = post(&handler, "not json", &headers).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(decode(&reply)["error"]["code"], -32700);

        let null_id = json!({
            "jsonrpc": "2.0", "id": null, "method": "tools/list",
            "params": {"_meta": meta(PROTOCOL_VERSION)},
        })
        .to_string();
        let reply = post(&handler, &null_id, &headers).await;
        assert_eq!(decode(&reply)["error"]["code"], -32600);

        let no_method = json!({"jsonrpc": "2.0", "id": 1}).to_string();
        let reply = post(&handler, &no_method, &headers).await;
        assert_eq!(decode(&reply)["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn ignores_session_and_resume_headers() {
        let store = Arc::new(FakeStore::default());
        store.seed("r1", 1000);
        let handler = Handler::new(store, "test");
        let body = request("tools/list", json!(1), Map::new());
        let mut headers = standard_headers("tools/list");
        headers.insert(
            HeaderName::from_static("mcp-session-id"),
            HeaderValue::from_static("legacy-session"),
        );
        headers.insert(
            HeaderName::from_static("last-event-id"),
            HeaderValue::from_static("42"),
        );
        let mut http_request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .body(Body::from(body))
            .expect("request");
        http_request.headers_mut().extend(headers);

        let response = handler.serve(http_request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("mcp-session-id").is_none());
    }

    #[tokio::test]
    async fn serves_an_http_request() {
        let store = Arc::new(FakeStore::default());
        store.seed("r1", 1000);
        let handler = Handler::new(store, "test");
        let body = request("server/discover", json!(1), Map::new());
        let http_request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header("Mcp-Method", "server/discover")
            .body(Body::from(body))
            .expect("request");
        let response = handler.serve(http_request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["result"]["resultType"], "complete");
    }
}
