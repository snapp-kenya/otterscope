//! OTLP/JSON decoding.
//!
//! The JSON mapping is not plain protobuf-JSON: trace and span IDs are hex
//! strings (the OTLP spec overrides protobuf's base64 rule for these two
//! fields), 64-bit integers are decimal strings, and enum values may be
//! either numbers or `SPAN_KIND_*`/`STATUS_CODE_*` names. Both camelCase and
//! snake_case field names are accepted, as the Go receiver does.

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde::de::{self, Deserializer};

use super::proto;

pub(crate) fn from_slice(bytes: &[u8]) -> Result<proto::ExportTraceServiceRequest> {
    let json: JsonRequest = serde_json::from_slice(bytes).context("decode OTLP/JSON payload")?;
    let mut request = json.into_proto();
    request.fill_defaults();
    request.validate()?;
    Ok(request)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonRequest {
    #[serde(default, alias = "resource_spans")]
    resource_spans: Vec<JsonResourceSpans>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonResourceSpans {
    #[serde(default)]
    resource: Option<JsonResource>,
    #[serde(default, alias = "scope_spans")]
    scope_spans: Vec<JsonScopeSpans>,
    #[serde(default, alias = "schema_url")]
    schema_url: String,
    #[serde(default, alias = "deprecated_scope_spans")]
    deprecated_scope_spans: Vec<JsonScopeSpans>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonResource {
    #[serde(default)]
    attributes: Vec<JsonKeyValue>,
    #[serde(
        default,
        alias = "dropped_attributes_count",
        deserialize_with = "de_u32"
    )]
    dropped_attributes_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonScopeSpans {
    #[serde(default)]
    scope: Option<JsonInstrumentationScope>,
    #[serde(default)]
    spans: Vec<JsonSpan>,
    #[serde(default, alias = "schema_url")]
    schema_url: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonInstrumentationScope {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    attributes: Vec<JsonKeyValue>,
    #[serde(
        default,
        alias = "dropped_attributes_count",
        deserialize_with = "de_u32"
    )]
    dropped_attributes_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonSpan {
    #[serde(default, alias = "trace_id", deserialize_with = "de_hex_id")]
    trace_id: Vec<u8>,
    #[serde(default, alias = "span_id", deserialize_with = "de_hex_id")]
    span_id: Vec<u8>,
    #[serde(default, alias = "trace_state")]
    trace_state: String,
    #[serde(default, alias = "parent_span_id", deserialize_with = "de_hex_id")]
    parent_span_id: Vec<u8>,
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "de_span_kind")]
    kind: i32,
    #[serde(default, alias = "start_time_unix_nano", deserialize_with = "de_u64")]
    start_time_unix_nano: u64,
    #[serde(default, alias = "end_time_unix_nano", deserialize_with = "de_u64")]
    end_time_unix_nano: u64,
    #[serde(default)]
    attributes: Vec<JsonKeyValue>,
    #[serde(
        default,
        alias = "dropped_attributes_count",
        deserialize_with = "de_u32"
    )]
    dropped_attributes_count: u32,
    #[serde(default)]
    events: Vec<JsonEvent>,
    #[serde(default, alias = "dropped_events_count", deserialize_with = "de_u32")]
    dropped_events_count: u32,
    #[serde(default)]
    links: Vec<JsonLink>,
    #[serde(default, alias = "dropped_links_count", deserialize_with = "de_u32")]
    dropped_links_count: u32,
    #[serde(default)]
    status: Option<JsonStatus>,
    #[serde(default, deserialize_with = "de_u32")]
    flags: u32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonEvent {
    #[serde(default, alias = "time_unix_nano", deserialize_with = "de_u64")]
    time_unix_nano: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    attributes: Vec<JsonKeyValue>,
    #[serde(
        default,
        alias = "dropped_attributes_count",
        deserialize_with = "de_u32"
    )]
    dropped_attributes_count: u32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonLink {
    #[serde(default, alias = "trace_id", deserialize_with = "de_hex_id")]
    trace_id: Vec<u8>,
    #[serde(default, alias = "span_id", deserialize_with = "de_hex_id")]
    span_id: Vec<u8>,
    #[serde(default, alias = "trace_state")]
    trace_state: String,
    #[serde(default)]
    attributes: Vec<JsonKeyValue>,
    #[serde(
        default,
        alias = "dropped_attributes_count",
        deserialize_with = "de_u32"
    )]
    dropped_attributes_count: u32,
    #[serde(default, deserialize_with = "de_u32")]
    flags: u32,
}

#[derive(Default, Deserialize)]
struct JsonKeyValue {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: Option<JsonAnyValue>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonAnyValue {
    #[serde(default, alias = "string_value")]
    string_value: Option<String>,
    #[serde(default, alias = "bool_value")]
    bool_value: Option<bool>,
    #[serde(default, alias = "int_value", deserialize_with = "de_i64_opt")]
    int_value: Option<i64>,
    #[serde(default, alias = "double_value", deserialize_with = "de_f64_opt")]
    double_value: Option<f64>,
    #[serde(default, alias = "array_value")]
    array_value: Option<JsonArrayValue>,
    #[serde(default, alias = "kvlist_value")]
    kvlist_value: Option<JsonKeyValueList>,
    #[serde(default, alias = "bytes_value", deserialize_with = "de_b64_opt")]
    bytes_value: Option<Vec<u8>>,
}

#[derive(Default, Deserialize)]
struct JsonArrayValue {
    #[serde(default)]
    values: Vec<JsonAnyValue>,
}

#[derive(Default, Deserialize)]
struct JsonKeyValueList {
    #[serde(default)]
    values: Vec<JsonKeyValue>,
}

#[derive(Default, Deserialize)]
struct JsonStatus {
    #[serde(default)]
    message: String,
    #[serde(default, deserialize_with = "de_status_code")]
    code: i32,
}

impl JsonRequest {
    fn into_proto(self) -> proto::ExportTraceServiceRequest {
        proto::ExportTraceServiceRequest {
            resource_spans: self.resource_spans.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<JsonResourceSpans> for proto::ResourceSpans {
    fn from(value: JsonResourceSpans) -> Self {
        Self {
            resource: value.resource.map(Into::into),
            scope_spans: value.scope_spans.into_iter().map(Into::into).collect(),
            schema_url: value.schema_url,
            deprecated_scope_spans: value
                .deprecated_scope_spans
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<JsonResource> for proto::Resource {
    fn from(value: JsonResource) -> Self {
        Self {
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            dropped_attributes_count: value.dropped_attributes_count,
        }
    }
}

impl From<JsonScopeSpans> for proto::ScopeSpans {
    fn from(value: JsonScopeSpans) -> Self {
        Self {
            scope: value.scope.map(Into::into),
            spans: value.spans.into_iter().map(Into::into).collect(),
            schema_url: value.schema_url,
        }
    }
}

impl From<JsonInstrumentationScope> for proto::InstrumentationScope {
    fn from(value: JsonInstrumentationScope) -> Self {
        Self {
            name: value.name,
            version: value.version,
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            dropped_attributes_count: value.dropped_attributes_count,
        }
    }
}

impl From<JsonSpan> for proto::Span {
    fn from(value: JsonSpan) -> Self {
        Self {
            trace_id: value.trace_id,
            span_id: value.span_id,
            trace_state: value.trace_state,
            parent_span_id: value.parent_span_id,
            name: value.name,
            kind: value.kind,
            start_time_unix_nano: value.start_time_unix_nano,
            end_time_unix_nano: value.end_time_unix_nano,
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            dropped_attributes_count: value.dropped_attributes_count,
            events: value.events.into_iter().map(Into::into).collect(),
            dropped_events_count: value.dropped_events_count,
            links: value.links.into_iter().map(Into::into).collect(),
            dropped_links_count: value.dropped_links_count,
            status: value.status.map(Into::into),
            flags: value.flags,
        }
    }
}

impl From<JsonEvent> for proto::Event {
    fn from(value: JsonEvent) -> Self {
        Self {
            time_unix_nano: value.time_unix_nano,
            name: value.name,
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            dropped_attributes_count: value.dropped_attributes_count,
        }
    }
}

impl From<JsonLink> for proto::Link {
    fn from(value: JsonLink) -> Self {
        Self {
            trace_id: value.trace_id,
            span_id: value.span_id,
            trace_state: value.trace_state,
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            dropped_attributes_count: value.dropped_attributes_count,
            flags: value.flags,
        }
    }
}

impl From<JsonKeyValue> for proto::KeyValue {
    fn from(value: JsonKeyValue) -> Self {
        Self {
            key: value.key,
            value: value.value.map(Into::into),
        }
    }
}

impl From<JsonAnyValue> for proto::AnyValue {
    fn from(value: JsonAnyValue) -> Self {
        use proto::any_value::Value;

        // Exactly one member is valid per OTLP/JSON; a sender that sets several
        // gets a stable precedence instead of a decode error.
        let value = if let Some(v) = value.string_value {
            Some(Value::String(v))
        } else if let Some(v) = value.bool_value {
            Some(Value::Bool(v))
        } else if let Some(v) = value.int_value {
            Some(Value::Int(v))
        } else if let Some(v) = value.double_value {
            Some(Value::Double(v))
        } else if let Some(v) = value.array_value {
            Some(Value::Array(proto::ArrayValue {
                values: v.values.into_iter().map(Into::into).collect(),
            }))
        } else if let Some(v) = value.kvlist_value {
            Some(Value::Kvlist(proto::KeyValueList {
                values: v.values.into_iter().map(Into::into).collect(),
            }))
        } else {
            value.bytes_value.map(Value::Bytes)
        };
        proto::AnyValue { value }
    }
}

impl From<JsonStatus> for proto::Status {
    fn from(value: JsonStatus) -> Self {
        Self {
            message: value.message,
            code: value.code,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IntOrString {
    Int(i64),
    Float(f64),
    Str(String),
}

fn de_hex_id<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    proto::hex_decode(&raw).map_err(de::Error::custom)
}

fn de_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(v) => u64::try_from(v).map_err(de::Error::custom),
        IntOrString::Float(_) => Err(de::Error::custom("expected an unsigned integer")),
        IntOrString::Str(v) => v.parse::<u64>().map_err(de::Error::custom),
    }
}

fn de_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(v) => u32::try_from(v).map_err(de::Error::custom),
        IntOrString::Float(_) => Err(de::Error::custom("expected an unsigned integer")),
        IntOrString::Str(v) => v.parse::<u32>().map_err(de::Error::custom),
    }
}

fn de_i64_opt<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<IntOrString>::deserialize(deserializer)? {
        None => Ok(None),
        Some(IntOrString::Int(v)) => Ok(Some(v)),
        Some(IntOrString::Float(_)) => Err(de::Error::custom("expected an integer")),
        Some(IntOrString::Str(v)) => v.parse::<i64>().map(Some).map_err(de::Error::custom),
    }
}

fn de_f64_opt<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<IntOrString>::deserialize(deserializer)? {
        None => Ok(None),
        Some(IntOrString::Int(v)) => Ok(Some(v as f64)),
        Some(IntOrString::Float(v)) => Ok(Some(v)),
        Some(IntOrString::Str(v)) => v.parse::<f64>().map(Some).map_err(de::Error::custom),
    }
}

fn de_b64_opt<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        None => Ok(None),
        Some(v) => BASE64.decode(v).map(Some).map_err(de::Error::custom),
    }
}

fn de_span_kind<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(v) => i32::try_from(v).map_err(de::Error::custom),
        IntOrString::Float(_) => Err(de::Error::custom("expected an integer")),
        IntOrString::Str(v) => match v.as_str() {
            "SPAN_KIND_UNSPECIFIED" => Ok(0),
            "SPAN_KIND_INTERNAL" => Ok(1),
            "SPAN_KIND_SERVER" => Ok(2),
            "SPAN_KIND_CLIENT" => Ok(3),
            "SPAN_KIND_PRODUCER" => Ok(4),
            "SPAN_KIND_CONSUMER" => Ok(5),
            other => Err(de::Error::custom(format!("unknown span kind {other:?}"))),
        },
    }
}

fn de_status_code<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    match IntOrString::deserialize(deserializer)? {
        IntOrString::Int(v) => i32::try_from(v).map_err(de::Error::custom),
        IntOrString::Float(_) => Err(de::Error::custom("expected an integer")),
        IntOrString::Str(v) => match v.as_str() {
            "STATUS_CODE_UNSET" => Ok(0),
            "STATUS_CODE_OK" => Ok(1),
            "STATUS_CODE_ERROR" => Ok(2),
            other => Err(de::Error::custom(format!("unknown status code {other:?}"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_snake_case_and_hex_ids() {
        let payload = r#"{
            "resource_spans": [{
                "scope_spans": [{
                    "spans": [{
                        "trace_id": "00000000000000000000000000000001",
                        "span_id": "0000000000000002",
                        "name": "x",
                        "start_time_unix_nano": "1000000000",
                        "end_time_unix_nano": 2000000000
                    }]
                }]
            }]
        }"#;
        let request = from_slice(payload.as_bytes()).unwrap();
        let span = &request.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(
            span.trace_id,
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(span.start_time_unix_nano, 1_000_000_000);
        assert_eq!(span.end_time_unix_nano, 2_000_000_000);
    }

    #[test]
    fn rejects_out_of_range_enum_numbers() {
        // Go decodes these into int32 and rejects a value that does not fit;
        // truncating would silently change the span kind or status.
        let too_big = i64::from(i32::MAX) + 1;
        assert!(serde_json::from_value::<JsonSpan>(serde_json::json!({"kind": too_big})).is_err());
        assert!(
            serde_json::from_value::<JsonStatus>(serde_json::json!({"code": too_big})).is_err()
        );
    }

    #[test]
    fn migrates_deprecated_scope_spans_only_when_needed() {
        let payload = r#"{
            "resourceSpans": [{
                "deprecatedScopeSpans": [{"spans": [{
                    "traceId": "00000000000000000000000000000001",
                    "spanId": "0000000000000002",
                    "name": "legacy",
                    "startTimeUnixNano": "1000000000"}]}]
            }]
        }"#;
        let request = from_slice(payload.as_bytes()).unwrap();
        let rs = &request.resource_spans[0];
        assert_eq!(rs.scope_spans[0].spans[0].name, "legacy");
        assert!(rs.deprecated_scope_spans.is_empty());

        let both = r#"{"resourceSpans":[{
            "scopeSpans":[{"spans":[{"traceId":"00000000000000000000000000000001",
                "spanId":"0000000000000002","name":"new","startTimeUnixNano":"1"}]}],
            "deprecatedScopeSpans":[{"spans":[{"traceId":"00000000000000000000000000000001",
                "spanId":"0000000000000003","name":"old","startTimeUnixNano":"1"}]}]
        }]}"#;
        let request = from_slice(both.as_bytes()).unwrap();
        let rs = &request.resource_spans[0];
        assert_eq!(rs.scope_spans.len(), 1);
        assert_eq!(rs.scope_spans[0].spans[0].name, "new");
        assert!(rs.deprecated_scope_spans.is_empty());
    }

    #[test]
    fn decodes_enum_names_and_every_any_value_variant() {
        let payload = r#"{
            "resourceSpans": [{
                "scopeSpans": [{
                    "spans": [{
                        "traceId": "00000000000000000000000000000001",
                        "spanId": "0000000000000002",
                        "name": "x",
                        "kind": "SPAN_KIND_CLIENT",
                        "startTimeUnixNano": "1000000000",
                        "status": {"code": "STATUS_CODE_ERROR", "message": "boom"},
                        "attributes": [
                            {"key": "bool", "value": {"boolValue": true}},
                            {"key": "double", "value": {"doubleValue": 1.5}},
                            {"key": "array", "value": {"arrayValue": {"values": [
                                {"intValue": 7}, {"stringValue": "s"}]}}},
                            {"key": "kvlist", "value": {"kvlistValue": {"values": [
                                {"key": "nested", "value": {"stringValue": "v"}}]}}},
                            {"key": "bytes", "value": {"bytesValue": "aGk="}}
                        ]
                    }]
                }]
            }]
        }"#;
        let request = from_slice(payload.as_bytes()).unwrap();
        let span = &request.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(span.kind, 3);
        assert_eq!(span.status.as_ref().unwrap().code, 2);

        use crate::ingest::proto::any_value::Value;
        let values: Vec<&Value> = span
            .attributes
            .iter()
            .map(|kv| kv.value.as_ref().unwrap().value.as_ref().unwrap())
            .collect();
        assert!(matches!(values[0], Value::Bool(true)));
        assert!(matches!(values[1], Value::Double(v) if (*v - 1.5).abs() < f64::EPSILON));
        assert!(matches!(values[2], Value::Array(array) if array.values.len() == 2));
        assert!(matches!(values[3], Value::Kvlist(list) if list.values[0].key == "nested"));
        assert!(matches!(values[4], Value::Bytes(bytes) if bytes == b"hi"));
    }

    #[test]
    fn rejects_short_ids() {
        let payload = r#"{"resourceSpans":[{"scopeSpans":[{"spans":[{
            "traceId": "abcd", "spanId": "0000000000000002", "name": "x",
            "startTimeUnixNano": "1"}]}]}]}"#;
        assert!(from_slice(payload.as_bytes()).is_err());
    }
}
