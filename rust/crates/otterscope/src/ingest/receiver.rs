//! `POST /v1/traces`: OTLP/HTTP with protobuf and JSON bodies, optional gzip,
//! per-project rate limiting and whole-batch work caps.
//!
//! Spec deviation, deliberate and inherited from the Go receiver: error bodies
//! are `text/plain` instead of `google.rpc.Status` protos. OTLP exporters act
//! on HTTP status codes; encoding Status would add a genproto dependency for
//! bodies nothing reads.

use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use flate2::read::GzDecoder;
use prost::Message;

use crate::basepath;

use super::proto;
use super::ratelimit::RateLimiter;
use super::{IngestResult, KeyResolver, Options, Sink, json, normalize};

const MAX_BODY_BYTES: usize = 32 << 20;

const CT_PROTO: &str = "application/x-protobuf";
const CT_JSON: &str = "application/json";

/// OTLP/HTTP trace receiver. Cheap to clone (its state is behind an `Arc`);
/// build it with [`Receiver::new`] and mount [`Receiver::router`].
#[derive(Clone)]
pub struct Receiver {
    inner: Arc<Inner>,
}

struct Inner {
    sink: Arc<dyn Sink>,
    resolve: KeyResolver,
    limit: Option<RateLimiter>,
    require_key: bool,
    max_spans: usize,
    max_trace_ids: usize,
    /// `None` means the configured base path could not be normalised, so
    /// nothing is mounted (all requests are 404s).
    base: Option<String>,
}

impl Receiver {
    pub fn new(opts: Options, sink: Arc<dyn Sink>, resolve: KeyResolver) -> Self {
        let base = basepath::normalize(&opts.base_path).ok();
        Self {
            inner: Arc::new(Inner {
                sink,
                resolve,
                limit: RateLimiter::new(opts.rate_per_sec, opts.burst),
                require_key: opts.require_key,
                max_spans: opts.max_spans(),
                max_trace_ids: opts.max_trace_ids(),
                base,
            }),
        }
    }

    pub fn router(self) -> Router {
        let base = self.inner.base.clone();
        let inner = self.inner;
        let traces = Router::new().route(
            "/v1/traces",
            post(move |request: Request| {
                let inner = Arc::clone(&inner);
                async move { handle(inner, request).await }
            }),
        );
        match base.as_deref() {
            None => Router::new(),
            Some("") => traces,
            Some(base) => Router::new().nest(base, traces),
        }
    }
}

/// Build the OTLP receiver at the listener root or under `options.base_path`.
pub fn router(options: Options, sink: Arc<dyn Sink>, resolve: KeyResolver) -> Router {
    Receiver::new(options, sink, resolve).router()
}

async fn handle(state: Arc<Inner>, request: Request) -> Response {
    let headers = request.headers().clone();
    let key = bearer_key(&headers);
    if state.require_key && key.is_empty() {
        return text_error(StatusCode::UNAUTHORIZED, "ingest key required");
    }
    // Rate-limit per project before doing any work.
    let resolve: &dyn Fn(&str) -> Option<String> = state.resolve.as_ref();
    let Some(project) = resolve(&key) else {
        return text_error(StatusCode::UNAUTHORIZED, "unknown ingest key");
    };
    if state
        .limit
        .as_ref()
        .is_some_and(|limit| !limit.allow(&project))
    {
        return text_error(StatusCode::TOO_MANY_REQUESTS, "ingest rate limit exceeded");
    }

    let Some(content_type) = media_type(&headers) else {
        return unsupported_content_type();
    };
    if content_type != CT_PROTO && content_type != CT_JSON {
        return unsupported_content_type();
    }

    let body = match read_body(request.into_body(), &headers).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let payload = if content_type == CT_PROTO {
        proto::decode(&body)
    } else {
        json::from_slice(&body)
    };
    let payload = match payload {
        Ok(payload) => payload,
        Err(error) => {
            return text_error(
                StatusCode::BAD_REQUEST,
                format!("malformed {content_type} payload: {error:#}"),
            );
        }
    };

    // Refuse an oversized batch before any normalization or database work: the
    // cost of a request must be bounded by these caps, not by its body.
    let span_count = payload.span_count();
    if span_count > state.max_spans {
        return text_error(
            StatusCode::BAD_REQUEST,
            format!(
                "batch rejected: {span_count} spans exceeds the maxSpansPerBatch limit of {}",
                state.max_spans
            ),
        );
    }
    let trace_ids = count_trace_ids(&payload, state.max_trace_ids);
    if trace_ids > state.max_trace_ids {
        return text_error(
            StatusCode::BAD_REQUEST,
            format!(
                "batch rejected: more than maxTraceIdsPerBatch distinct trace ids (limit {})",
                state.max_trace_ids
            ),
        );
    }

    let raw = match proto::compress(&payload) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::error!(error = %error, "ingest: compressing raw batch failed");
            return text_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to store traces");
        }
    };
    let mut steps = normalize::normalize(&payload);
    for step in &mut steps {
        step.project = project.clone();
    }

    // The sink writes to the single SQLite connection; keep it off the async
    // runtime's worker threads.
    let sink = Arc::clone(&state.sink);
    let task = tokio::task::spawn_blocking(move || sink.consume(&project, &raw, &steps)).await;
    let result = match task {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::error!(error = %error, "ingest: sink failed");
            return text_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to store traces");
        }
        Err(error) => {
            tracing::error!(error = %error, "ingest: sink task failed");
            return text_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to store traces");
        }
    };

    write_export_response(&content_type, &result)
}

/// Count distinct trace IDs, stopping as soon as the limit is exceeded so a
/// hostile batch cannot make the check itself expensive.
fn count_trace_ids(request: &proto::ExportTraceServiceRequest, limit: usize) -> usize {
    let mut ids = HashSet::new();
    for resource_spans in &request.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                if proto::is_empty_id(&span.trace_id) {
                    continue;
                }
                ids.insert(span.trace_id.clone());
                if ids.len() > limit {
                    return ids.len();
                }
            }
        }
    }
    ids.len()
}

fn bearer_key(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.strip_prefix("Bearer ").unwrap_or(value).to_string())
        .unwrap_or_default()
}

fn media_type(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let (kind, params) = match raw.split_once(';') {
        Some((kind, params)) => (kind, Some(params)),
        None => (raw, None),
    };
    let kind = kind.trim().to_ascii_lowercase();
    if kind.is_empty() || !kind.contains('/') {
        return None;
    }
    if let Some(params) = params {
        for param in params.split(';') {
            let param = param.trim();
            if param.is_empty() || !param.contains('=') {
                return None;
            }
        }
    }
    Some(kind)
}

enum ReadError {
    TooLarge,
    BadGzip(String),
}

impl ReadError {
    fn into_response(self) -> Response {
        match self {
            ReadError::TooLarge => text_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "http: request body too large",
            ),
            ReadError::BadGzip(message) => text_error(StatusCode::BAD_REQUEST, message),
        }
    }
}

async fn read_body(body: Body, headers: &HeaderMap) -> Result<Vec<u8>, ReadError> {
    let raw = axum::body::to_bytes(body, MAX_BODY_BYTES + 1)
        .await
        .map_err(|_| ReadError::TooLarge)?;
    if raw.len() > MAX_BODY_BYTES {
        return Err(ReadError::TooLarge);
    }
    let gzipped = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        == Some("gzip");
    if !gzipped {
        return Ok(raw.to_vec());
    }
    let mut payload = Vec::new();
    GzDecoder::new(raw.as_ref())
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut payload)
        .map_err(|error| ReadError::BadGzip(format!("bad gzip body: {error}")))?;
    // Cap the decompressed size too, or a tiny gzip bomb bypasses the body
    // limit.
    if payload.len() > MAX_BODY_BYTES {
        return Err(ReadError::TooLarge);
    }
    Ok(payload)
}

fn unsupported_content_type() -> Response {
    text_error(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported content type: use application/x-protobuf or application/json",
    )
}

fn text_error(status: StatusCode, message: impl Into<String>) -> Response {
    let mut body = message.into();
    body.push('\n');
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}

fn write_export_response(content_type: &str, result: &IngestResult) -> Response {
    let partial_success = if result.rejected_spans > 0 {
        proto::ExportTracePartialSuccess {
            rejected_spans: result.rejected_spans,
            error_message: result.message.clone(),
        }
    } else {
        proto::ExportTracePartialSuccess::default()
    };
    let body = if content_type == CT_PROTO {
        proto::ExportTraceServiceResponse {
            partial_success: Some(partial_success),
        }
        .encode_to_vec()
    } else {
        match serde_json::to_vec(&JsonExportResponse::from(&partial_success)) {
            Ok(body) => body,
            Err(error) => {
                tracing::error!(error = %error, "ingest: encoding OTLP response failed");
                return text_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to encode response",
                );
            }
        }
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type.to_string())],
        body,
    )
        .into_response()
}

#[derive(serde::Serialize)]
struct JsonExportResponse {
    #[serde(rename = "partialSuccess")]
    partial_success: JsonPartialSuccess,
}

#[derive(serde::Serialize)]
struct JsonPartialSuccess {
    #[serde(rename = "rejectedSpans", skip_serializing_if = "String::is_empty")]
    rejected_spans: String,
    #[serde(rename = "errorMessage", skip_serializing_if = "String::is_empty")]
    error_message: String,
}

impl From<&proto::ExportTracePartialSuccess> for JsonExportResponse {
    fn from(value: &proto::ExportTracePartialSuccess) -> Self {
        Self {
            partial_success: JsonPartialSuccess {
                // int64 is a decimal string in OTLP/JSON.
                rejected_spans: if value.rejected_spans == 0 {
                    String::new()
                } else {
                    value.rejected_spans.to_string()
                },
                error_message: value.error_message.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use http_body_util::BodyExt;
    use prost::Message as _;
    use tower::ServiceExt;

    use crate::ingest::testutil;
    use crate::ingest::{
        DEFAULT_MAX_SPANS_PER_BATCH, DEFAULT_MAX_TRACE_IDS_PER_BATCH, IngestResult,
    };
    use crate::model::Step;

    use super::{CT_JSON, CT_PROTO};
    use crate::ingest::{KeyResolver, Options, Receiver, Sink};

    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<(String, usize)>>,
        result: IngestResult,
        error: Option<String>,
        raw: Mutex<Option<Vec<u8>>>,
    }

    impl Sink for RecordingSink {
        fn consume(
            &self,
            project: &str,
            raw: &[u8],
            steps: &[Step],
        ) -> anyhow::Result<IngestResult> {
            if let Some(error) = &self.error {
                return Err(anyhow::anyhow!("{error}"));
            }
            self.seen
                .lock()
                .unwrap()
                .push((project.to_string(), steps.len()));
            *self.raw.lock().unwrap() = Some(raw.to_vec());
            Ok(self.result.clone())
        }
    }

    fn resolver() -> KeyResolver {
        Arc::new(|key: &str| match key {
            "" => Some("default".to_string()),
            "valid-key" => Some("prod".to_string()),
            _ => None,
        })
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    async fn post(
        router: Router,
        content_type: &str,
        body: Vec<u8>,
        gzipped: bool,
        authorization: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let body = if gzipped { gzip(&body) } else { body };
        let mut request = Request::builder()
            .method("POST")
            .uri("/v1/traces")
            .header("content-type", content_type);
        if gzipped {
            request = request.header("content-encoding", "gzip");
        }
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        }
        let response = router
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, bytes)
    }

    fn as_sink(sink: &Arc<RecordingSink>) -> Arc<dyn Sink> {
        Arc::clone(sink) as Arc<dyn Sink>
    }

    fn sink_router(sink: Arc<RecordingSink>) -> Router {
        crate::ingest::router(Options::default(), sink, resolver())
    }

    #[tokio::test]
    async fn json_ingest_succeeds() {
        let sink = Arc::new(RecordingSink::default());
        let (status, headers, body) = post(
            sink_router(Arc::clone(&sink)),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "body: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(headers.get("content-type").unwrap(), CT_JSON);
        assert_eq!(
            sink.seen.lock().unwrap().as_slice(),
            &[("default".to_string(), 3)]
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"partialSuccess": {}})
        );
    }

    #[tokio::test]
    async fn proto_ingest_succeeds() {
        let sink = Arc::new(RecordingSink::default());
        let (status, headers, body) = post(
            sink_router(Arc::clone(&sink)),
            CT_PROTO,
            testutil::fixture("pydantic_ai_chat.pb"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers.get("content-type").unwrap(), CT_PROTO);
        assert_eq!(
            sink.seen.lock().unwrap().as_slice(),
            &[("default".to_string(), 3)]
        );
        let response =
            crate::ingest::proto::ExportTraceServiceResponse::decode(body.as_slice()).unwrap();
        assert_eq!(response.partial_success.unwrap().rejected_spans, 0);
    }

    #[tokio::test]
    async fn gzip_json_ingest_succeeds() {
        let sink = Arc::new(RecordingSink::default());
        let (status, _, _) = post(
            sink_router(Arc::clone(&sink)),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            true,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.seen.lock().unwrap()[0].1, 3);
    }

    #[tokio::test]
    async fn every_captured_fixture_ingests() {
        let fixtures = [
            ("pydantic_ai_chat.json", 3),
            ("genai_experimental_chat.json", 4),
            ("openai_agents_openinference.json", 5),
            ("vercel_ai_sdk.json", 3),
            ("claude_agent_sdk.json", 6),
        ];
        for (name, expected_steps) in fixtures {
            let sink = Arc::new(RecordingSink::default());
            let (status, _, body) = post(
                sink_router(Arc::clone(&sink)),
                CT_JSON,
                testutil::fixture(name),
                false,
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{name}: {}",
                String::from_utf8_lossy(&body)
            );
            assert_eq!(sink.seen.lock().unwrap()[0].1, expected_steps, "{name}");
        }
    }

    #[tokio::test]
    async fn malformed_payloads_are_400() {
        for content_type in [CT_JSON, CT_PROTO] {
            let sink = Arc::new(RecordingSink::default());
            let (status, _, _) = post(
                sink_router(sink),
                content_type,
                br#"{"resourceSpans": [{]"#.to_vec(),
                false,
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "content type {content_type}"
            );
        }
    }

    #[tokio::test]
    async fn bad_gzip_is_400() {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/traces")
            .header("content-type", CT_JSON)
            .header("content-encoding", "gzip")
            .body(Body::from("not gzip"))
            .unwrap();
        let response = sink_router(Arc::new(RecordingSink::default()))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn content_type_parameters_are_accepted() {
        let sink = Arc::new(RecordingSink::default());
        let (status, _, _) = post(
            sink_router(sink),
            "application/json; charset=utf-8",
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn unusable_base_path_mounts_nothing() {
        let options = Options {
            base_path: "/a//b".to_string(),
            ..Default::default()
        };
        let router = crate::ingest::router(options, Arc::new(RecordingSink::default()), resolver());
        let request = Request::builder()
            .method("POST")
            .uri("/a/b/v1/traces")
            .header("content-type", CT_JSON)
            .body(Body::from(testutil::fixture("pydantic_ai_chat.json")))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unsupported_content_type_is_415() {
        let (status, _, _) = post(
            sink_router(Arc::new(RecordingSink::default())),
            "text/plain",
            b"hi".to_vec(),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn sink_error_is_500() {
        let sink = Arc::new(RecordingSink {
            error: Some("disk full".to_string()),
            ..Default::default()
        });
        let (status, _, _) = post(
            sink_router(sink),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn fixture_round_trip_matches_go_decoding() {
        // The .pb fixture is the protobuf twin of the .json fixture. Go's pdata
        // encoder emits fields in descending tag order while prost emits them
        // ascending, so compare decoded batches rather than bytes.
        let from_json = testutil::request_from_fixture("pydantic_ai_chat.json");
        let from_proto =
            crate::ingest::proto::decode(&testutil::fixture("pydantic_ai_chat.pb")).unwrap();
        assert_eq!(from_proto, from_json);
        assert_eq!(from_proto.span_count(), 3);
    }

    #[tokio::test]
    async fn auth_resolves_projects() {
        let sink = Arc::new(RecordingSink::default());
        let router = sink_router(Arc::clone(&sink));
        let fixture = testutil::fixture("pydantic_ai_chat.json");

        let (status, _, _) = post(
            router.clone(),
            CT_JSON,
            fixture.clone(),
            false,
            Some("Bearer valid-key"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.seen.lock().unwrap().last().unwrap().0, "prod");

        let (status, _, _) = post(
            router.clone(),
            CT_JSON,
            fixture.clone(),
            false,
            Some("Bearer wrong"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _, _) = post(router, CT_JSON, fixture, false, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.seen.lock().unwrap().last().unwrap().0, "default");
    }

    #[tokio::test]
    async fn require_key_rejects_keyless_ingest() {
        let sink = Arc::new(RecordingSink::default());
        let options = Options {
            require_key: true,
            ..Default::default()
        };
        let router = crate::ingest::router(options, as_sink(&sink), resolver());
        let (status, _, _) = post(
            router.clone(),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(sink.seen.lock().unwrap().is_empty());

        let (status, _, _) = post(
            router,
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            Some("Bearer valid-key"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sink.seen.lock().unwrap()[0].0, "prod");
    }

    #[tokio::test]
    async fn span_limit_refuses_whole_batch() {
        let sink = Arc::new(RecordingSink::default());
        let request = testutil::request_with_spans(
            "svc",
            vec![
                testutil::span("a", "0000000000000001", 1),
                testutil::span("b", "0000000000000002", 1),
                testutil::span("c", "0000000000000003", 1),
            ],
        );
        let options = Options {
            max_spans: 2,
            ..Default::default()
        };
        let router = crate::ingest::router(options, as_sink(&sink), resolver());
        let (status, _, body) = post(router, CT_PROTO, request.encode_to_vec(), false, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("maxSpansPerBatch"), "body: {body}");
        assert!(body.contains('2'), "body: {body}");
        assert!(sink.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn trace_id_limit_refuses_whole_batch() {
        let options = Options {
            max_trace_ids: 2,
            ..Default::default()
        };
        let request = testutil::request_with_spans(
            "svc",
            vec![
                testutil::span("a", "0000000000000001", 1),
                testutil::span("b", "0000000000000002", 2),
                testutil::span("c", "0000000000000003", 3),
            ],
        );
        let router = crate::ingest::router(
            options.clone(),
            Arc::new(RecordingSink::default()),
            resolver(),
        );
        let (status, _, body) = post(router, CT_PROTO, request.encode_to_vec(), false, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("maxTraceIdsPerBatch"), "body: {body}");

        // At the limit the same shapes are accepted: the cap must not block
        // ordinary multi-run batches.
        let request = testutil::request_with_spans(
            "svc",
            vec![
                testutil::span("a", "0000000000000001", 1),
                testutil::span("b", "0000000000000002", 2),
            ],
        );
        let router = crate::ingest::router(options, Arc::new(RecordingSink::default()), resolver());
        let (status, _, _) = post(router, CT_PROTO, request.encode_to_vec(), false, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejected_spans_are_reported_as_partial_success() {
        let sink = Arc::new(RecordingSink {
            result: IngestResult {
                rejected_spans: 2,
                message: "2 span(s) rejected: different service".to_string(),
            },
            ..Default::default()
        });
        let (status, _, body) = post(
            sink_router(sink),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let response: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(response["partialSuccess"]["rejectedSpans"], "2");
        assert!(
            response["partialSuccess"]["errorMessage"]
                .as_str()
                .unwrap()
                .contains("different service")
        );
    }

    #[tokio::test]
    async fn router_normalizes_base_paths() {
        for base in [
            "/otterscope",
            "/otterscope/",
            "otterscope",
            "  /otterscope  ",
        ] {
            let sink = Arc::new(RecordingSink::default());
            let options = Options {
                base_path: base.to_string(),
                ..Default::default()
            };
            let router = crate::ingest::router(options, as_sink(&sink), resolver());
            let request = Request::builder()
                .method("POST")
                .uri("/otterscope/v1/traces")
                .header("content-type", CT_JSON)
                .body(Body::from(testutil::fixture("pydantic_ai_chat.json")))
                .unwrap();
            let response = router.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "base {base:?}");
            assert_eq!(sink.seen.lock().unwrap()[0].1, 3, "base {base:?}");
        }
    }

    #[tokio::test]
    async fn root_mount_does_not_serve_prefixed_path() {
        let request = Request::builder()
            .method("POST")
            .uri("/otterscope/v1/traces")
            .header("content-type", CT_JSON)
            .body(Body::from(testutil::fixture("pydantic_ai_chat.json")))
            .unwrap();
        let response = sink_router(Arc::new(RecordingSink::default()))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn base_mount_does_not_serve_root_and_still_decodes() {
        let options = Options {
            base_path: "/otterscope".to_string(),
            ..Default::default()
        };
        let sink = Arc::new(RecordingSink::default());
        let router = Receiver::new(options, as_sink(&sink), resolver());

        let root = Request::builder()
            .method("POST")
            .uri("/v1/traces")
            .header("content-type", CT_JSON)
            .body(Body::from(testutil::fixture("pydantic_ai_chat.json")))
            .unwrap();
        let response = router.router().oneshot(root).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let malformed = Request::builder()
            .method("POST")
            .uri("/otterscope/v1/traces")
            .header("content-type", CT_JSON)
            .body(Body::from(br#"{"resourceSpans": [{]"#.to_vec()))
            .unwrap();
        let router = Receiver::new(
            Options {
                base_path: "/otterscope".to_string(),
                ..Default::default()
            },
            sink,
            resolver(),
        );
        let response = router.router().oneshot(malformed).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn defaults_match_go() {
        let options = Options::default();
        assert_eq!(options.max_spans() as i64, DEFAULT_MAX_SPANS_PER_BATCH);
        assert_eq!(
            options.max_trace_ids() as i64,
            DEFAULT_MAX_TRACE_IDS_PER_BATCH
        );
    }

    #[tokio::test]
    async fn raw_batch_is_retained_for_renormalization() {
        let sink = Arc::new(RecordingSink::default());
        let (status, _, _) = post(
            sink_router(Arc::clone(&sink)),
            CT_JSON,
            testutil::fixture("pydantic_ai_chat.json"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let raw = sink.raw.lock().unwrap().clone().expect("raw batch stored");
        let replayed = crate::ingest::renormalize_batch(&raw).unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(
            replayed
                .iter()
                .map(|step| step.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "invoke_agent support-agent",
                "chat claude-sonnet-5",
                "execute_tool get_ticket"
            ]
        );
    }

    #[tokio::test]
    async fn rate_limit_denies_after_burst() {
        let sink = Arc::new(RecordingSink::default());
        let options = Options {
            rate_per_sec: 0.001,
            burst: 1.0,
            ..Default::default()
        };
        let router = crate::ingest::router(options, as_sink(&sink), resolver());
        let fixture = testutil::fixture("pydantic_ai_chat.json");
        let (first, _, _) = post(router.clone(), CT_JSON, fixture.clone(), false, None).await;
        assert_eq!(first, StatusCode::OK);
        let (second, _, body) = post(router, CT_JSON, fixture, false, None).await;
        assert_eq!(second, StatusCode::TOO_MANY_REQUESTS);
        assert!(String::from_utf8_lossy(&body).contains("rate limit"));
        assert_eq!(sink.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn proto_partial_success_reports_rejections() {
        let sink = Arc::new(RecordingSink {
            result: IngestResult::with_rejections(1),
            ..Default::default()
        });
        let (status, _, body) = post(
            sink_router(sink),
            CT_PROTO,
            testutil::fixture("pydantic_ai_chat.pb"),
            false,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let response =
            crate::ingest::proto::ExportTraceServiceResponse::decode(body.as_slice()).unwrap();
        let partial = response.partial_success.unwrap();
        assert_eq!(partial.rejected_spans, 1);
        assert!(partial.error_message.contains("different service"));
    }
}
