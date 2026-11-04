//! HTTP API, auth guard, metrics and the embedded UI (port of `internal/server`).
//!
//! One listener serves three things: the JSON read/admin API, the Prometheus
//! text endpoint, and the embedded frontend. The gates run in the order the Go
//! server composes them: the mount prefix at the very edge, then bearer auth,
//! then the browser guard, then routing.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Method, Request, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use crate::basepath;
use crate::config::Config;
use crate::store::{Scope, Store};

pub(crate) mod api;
mod backend;
#[cfg(not(test))]
mod backend_real;
pub mod evaluate;
mod evaluator;
mod guard;
mod metrics;
mod sink;
mod stream;
#[cfg(test)]
mod tests;
mod ui;

pub use backend::{
    AlertRecord, ApiBackend, AuditRecord, BackendError, BackendResult, CreatedTokenRecord,
    EvaluateOptions, EvaluateReport, NewAlert, ShareRecord, TokenRecord, ViewRecord,
};
pub use evaluator::Evaluator;
pub use sink::{StoreSink, price_steps};
pub use stream::Hub;

/// Default cap on one JSON request body. The API only ever reads small
/// objects; a larger body is a mistake or an attempt to hold memory.
pub(crate) const MAX_BODY_BYTES: usize = 1 << 20;

/// Mounted state. `S` defaults to the real store so `AppState { store }` keeps
/// working for embedders.
pub struct AppState<S = Store> {
    pub store: S,
}

/// Everything startup decides for the HTTP layer: auth, mount, version, the
/// browser allow-list snapshot, and judge pricing rules.
#[derive(Debug, Clone)]
pub struct Options {
    pub read_auth: bool,
    pub base_path: String,
    pub version: String,
    pub allowed_origins: String,
    pub judge_models: Vec<String>,
    pub judge_url: String,
    pub judge_key: String,
    pub csv_export_limit: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            read_auth: false,
            base_path: String::new(),
            version: crate::VERSION.to_string(),
            allowed_origins: String::new(),
            judge_models: Vec::new(),
            judge_url: "https://api.openai.com/v1".to_string(),
            judge_key: String::new(),
            csv_export_limit: 10_000,
        }
    }
}

impl Options {
    pub fn from_config(config: &Config) -> Self {
        Options {
            read_auth: config.read_auth,
            base_path: config.base_path.clone(),
            allowed_origins: config.allowed_origins.clone(),
            judge_models: config.judge_models.clone(),
            judge_url: config.judge_url.clone(),
            judge_key: config.judge_key.clone(),
            ..Options::default()
        }
    }
}

/// Build the production router over a shared store and a hub the ingest sink
/// also broadcasts on.
#[cfg(not(test))]
pub fn router_with(state: AppState<Arc<Store>>, options: Options, hub: Hub) -> Router {
    let backend = backend_real::RealBackend::new(state.store, &options);
    build_with_hub(Arc::new(backend), options, hub)
}

/// Build the router over any backend. Used by `serve` and by the test suite,
/// which substitutes an in-memory backend.
pub fn build(backend: Arc<dyn ApiBackend>, options: Options) -> Router {
    build_with_hub(backend, options, Hub::new())
}

/// Build the router with a hub the ingest sink can broadcast on.
pub fn build_with_hub(backend: Arc<dyn ApiBackend>, mut options: Options, hub: Hub) -> Router {
    if let Ok(normalized) = basepath::normalize(&options.base_path) {
        options.base_path = normalized;
    }
    let allow = Arc::new(guard::AllowList::parse(&options.allowed_origins));
    let mcp = backend.mcp_router();
    let state = ServerState {
        backend,
        options: Arc::new(options),
        allow,
        hub,
    };
    let mut api = api::router().with_state(state.clone());
    api = match mcp {
        Some(mcp) => api.merge(mcp),
        None => api.route("/mcp", post(mcp_unavailable)),
    };
    let app = App { state, api };
    let base_path = app.state.options.base_path.clone();
    Router::new()
        .fallback(dispatch)
        .with_state(app)
        .layer(middleware::from_fn(move |req, next| {
            let base_path = base_path.clone();
            async move { harden(base_path, req, next).await }
        }))
}

/// Stamp the headers every response carries and turn axum's plain `413` into
/// the JSON envelope. The middleware wraps the fallback too, so auth failures
/// and prefix misses are hardened as well.
async fn harden(base_path: String, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let api = api_path(&base_path, &path);
    let mut response = next.run(req).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        response = json_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    }
    guard::stamp_security_headers(response.headers_mut());
    if api {
        insert_header(response.headers_mut(), header::CACHE_CONTROL, "no-store");
    }
    response
}

/// Whether a request path addresses the JSON API or MCP, including paths that
/// sit outside the mount prefix (they still must not be cached as an API miss).
fn api_path(base_path: &str, path: &str) -> bool {
    let mounted = basepath::strip(base_path, path).unwrap_or(path);
    mounted == "/api"
        || mounted.starts_with("/api/")
        || mounted == "/mcp"
        || mounted.starts_with("/mcp/")
}

#[derive(Clone)]
pub(crate) struct ServerState {
    backend: Arc<dyn ApiBackend>,
    options: Arc<Options>,
    allow: Arc<guard::AllowList>,
    hub: Hub,
}

#[derive(Clone)]
struct App {
    state: ServerState,
    api: Router,
}

async fn mcp_unavailable() -> Response {
    json_error(
        StatusCode::NOT_IMPLEMENTED,
        "mcp is not available in this build",
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AuthInfo {
    pub read_auth: bool,
    pub scope: Option<Scope>,
}

/// The edge: mount, auth, guard, then routing or the SPA.
async fn dispatch(State(app): State<App>, mut req: Request<Body>) -> Response {
    let original = req.uri().path().to_string();
    let Some(mounted) = basepath::strip(&app.state.options.base_path, &original) else {
        return plain_text(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "404 page not found\n",
        );
    };
    let path = mounted.to_string();
    if guard::unclean(&path) {
        return redirect(&app.state.options.base_path, &path, req.uri().query());
    }
    if path != original {
        rewrite_path(&mut req, &path);
    }

    let mut auth = AuthInfo {
        read_auth: app.state.options.read_auth,
        scope: None,
    };
    if auth.read_auth && auth_protected(&path) {
        match presented_token(&req, &path)
            .and_then(|token| app.state.backend.authenticate_token(&token))
        {
            Some(token) => auth.scope = Some(token.scope),
            None => return json_error(StatusCode::UNAUTHORIZED, "read token required"),
        }
    }
    req.extensions_mut().insert(auth);

    match guard::check(&app.state.allow, &req, &path) {
        Some(refusal) => refusal,
        None if is_ui_path(&path) => {
            ui::response(&path, req.method(), &app.state.options.base_path)
        }
        None => app
            .api
            .clone()
            .oneshot(req)
            .await
            .unwrap_or_else(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")),
    }
}

/// `/healthz` stays open so a liveness probe needs no credential; so do public
/// shares. `/metrics` is an inventory of the deployment and takes a read token.
fn auth_protected(path: &str) -> bool {
    if path == "/healthz" || path.starts_with("/api/shared/") {
        return false;
    }
    path.starts_with("/api/") || path == "/mcp" || path == "/metrics"
}

/// Paths that never reach the SPA: the API, MCP, metrics and the health check.
fn is_ui_path(path: &str) -> bool {
    !(path == "/api"
        || path.starts_with("/api/")
        || path == "/mcp"
        || path == "/metrics"
        || path == "/healthz")
}

fn presented_token(req: &Request<Body>, path: &str) -> Option<String> {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header);
    if !token.is_empty() {
        return Some(token.to_string());
    }
    // EventSource cannot set headers, so the SSE endpoint is the one route that
    // may carry its token in the query string.
    if path == "/api/stream" {
        return query_param(req.uri().query(), "token").map(str::to_string);
    }
    None
}

fn rewrite_path(req: &mut Request<Body>, path: &str) {
    let query = req
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    if let Ok(uri) = format!("{path}{query}").parse::<Uri>() {
        *req.uri_mut() = uri;
    }
}

fn redirect(base_path: &str, path: &str, query: Option<&str>) -> Response {
    let clean = guard::clean_preserving_slash(path);
    let mut location = format!("{base_path}{clean}");
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        location.push('?');
        location.push_str(query);
    }
    match Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(header::LOCATION, location)
        .body(Body::empty())
    {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, "redirect response");
            internal_error()
        }
    }
}

/// Admin-only routes are refused before their handler runs. With read auth off
/// the whole API is open, so the gate is a no-op.
pub(crate) async fn admin_only(req: Request<Body>, next: Next) -> Response {
    let auth = req
        .extensions()
        .get::<AuthInfo>()
        .copied()
        .unwrap_or_default();
    if auth.read_auth && auth.scope != Some(Scope::Admin) {
        return json_error(StatusCode::FORBIDDEN, "admin token required");
    }
    next.run(req).await
}

#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            insert_header(
                response.headers_mut(),
                header::CONTENT_TYPE,
                "application/json",
            );
            insert_header(response.headers_mut(), header::CACHE_CONTROL, "no-store");
            response
        }
        Err(err) => {
            tracing::error!(error = %err, "encode JSON response");
            internal_error()
        }
    }
}

pub(crate) fn json_error(status: StatusCode, message: &str) -> Response {
    json_response(status, &ErrorBody { error: message })
}

pub(crate) fn plain_text(status: StatusCode, content_type: &str, body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    insert_header(response.headers_mut(), header::CONTENT_TYPE, content_type);
    response
}

pub(crate) fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) fn internal_error() -> Response {
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

pub(crate) fn insert_header(
    headers: &mut axum::http::HeaderMap,
    name: header::HeaderName,
    value: &str,
) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

/// Decode an `application/x-www-form-urlencoded` query string. First occurrence
/// wins, matching Go's `url.Values.Get`.
pub(crate) fn query_map(raw: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(raw) = raw else {
        return map;
    };
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        map.entry(percent_decode(key))
            .or_insert_with(|| percent_decode(value));
    }
    map
}

/// Parse an integer query param with a default and clamping, as the Go
/// handlers do: nonsense falls back to the default, never to an error.
pub(crate) fn query_int(
    query: &HashMap<String, String>,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
) -> i64 {
    match query.get(key).and_then(|value| value.parse::<i64>().ok()) {
        Some(value) => value.clamp(min, max),
        None => default,
    }
}

pub(crate) fn query_param<'a>(raw: Option<&'a str>, key: &str) -> Option<&'a str> {
    let raw = raw?;
    for pair in raw.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(name) == key {
            return Some(value);
        }
    }
    None
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Deserialise a JSON body, ignoring an empty body as Go's decoder would.
pub(crate) fn decode_json<T: for<'de> Deserialize<'de>>(
    body: &[u8],
) -> Result<T, serde_json::Error> {
    serde_json::from_slice(body)
}

/// HTTP method used by the UI fallback: only reads reach the frontend.
pub(crate) fn is_read_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}
