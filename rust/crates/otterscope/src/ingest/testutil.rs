//! Shared helpers for the ingest test suite.

use std::path::PathBuf;

use super::json;
use super::proto;
use super::proto::any_value::Value;

pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()))
}

pub(crate) fn request_from_fixture(name: &str) -> proto::ExportTraceServiceRequest {
    let bytes = fixture(name);
    if name.ends_with(".pb") {
        proto::decode(&bytes).unwrap()
    } else {
        json::from_slice(&bytes).unwrap()
    }
}

pub(crate) enum Attr<'a> {
    Str(&'a str),
    Int(i64),
}

impl Attr<'_> {
    fn into_value(self) -> proto::AnyValue {
        proto::AnyValue {
            value: Some(match self {
                Attr::Str(value) => Value::String(value.to_string()),
                Attr::Int(value) => Value::Int(value),
            }),
        }
    }
}

pub(crate) fn key_value(key: &str, value: Attr) -> proto::KeyValue {
    proto::KeyValue {
        key: key.to_string(),
        value: Some(value.into_value()),
    }
}

/// A one-resource, one-span batch matching the Go table tests.
pub(crate) fn single_span(
    operation: Option<&str>,
    extra: &[(&str, Attr)],
) -> proto::ExportTraceServiceRequest {
    let mut attributes = Vec::new();
    if let Some(operation) = operation {
        attributes.push(key_value("gen_ai.operation.name", Attr::Str(operation)));
    }
    for (key, value) in extra {
        attributes.push(proto::KeyValue {
            key: (*key).to_string(),
            value: Some(match value {
                Attr::Str(value) => proto::AnyValue {
                    value: Some(Value::String((*value).to_string())),
                },
                Attr::Int(value) => proto::AnyValue {
                    value: Some(Value::Int(*value)),
                },
            }),
        });
    }
    request_with_spans(
        "",
        vec![proto::Span {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            name: "test-span".to_string(),
            start_time_unix_nano: 1_000_000_000,
            end_time_unix_nano: 2_000_000_000,
            attributes,
            status: Some(proto::Status::default()),
            ..proto::Span::default()
        }],
    )
}

/// A span in a one-resource batch, as an exporter emits it. `trace_index`
/// selects a distinct trace ID so trace-ID caps can be exercised.
pub(crate) fn span(name: &str, span_id_hex: &str, trace_index: u8) -> proto::Span {
    let mut trace_id = vec![0u8; 16];
    trace_id[15] = trace_index;
    proto::Span {
        trace_id,
        span_id: proto::hex_decode(span_id_hex).unwrap(),
        name: name.to_string(),
        start_time_unix_nano: 1_000_000_000,
        end_time_unix_nano: 1_100_000_000,
        status: Some(proto::Status::default()),
        ..proto::Span::default()
    }
}

pub(crate) fn request_with_spans(
    service: &str,
    spans: Vec<proto::Span>,
) -> proto::ExportTraceServiceRequest {
    let attributes = if service.is_empty() {
        Vec::new()
    } else {
        vec![key_value("service.name", Attr::Str(service))]
    };
    proto::ExportTraceServiceRequest {
        resource_spans: vec![proto::ResourceSpans {
            resource: Some(proto::Resource {
                attributes,
                ..proto::Resource::default()
            }),
            scope_spans: vec![proto::ScopeSpans {
                scope: Some(proto::InstrumentationScope::default()),
                spans,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
            deprecated_scope_spans: Vec::new(),
        }],
    }
}
