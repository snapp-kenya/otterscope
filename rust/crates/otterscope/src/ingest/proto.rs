//! Hand-written prost mirrors of the OTLP trace messages.
//!
//! Only the wire shapes the receiver needs live here; `opentelemetry-proto`
//! would drag a proto compiler, tonic and the OpenTelemetry SDK into the build
//! for a handful of structs. Field numbers and defaults follow
//! `opentelemetry/proto/trace/v1/trace.proto` and `common/v1/common.proto`.

use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ExportTraceServiceRequest {
    #[prost(message, repeated, tag = "1")]
    pub resource_spans: Vec<ResourceSpans>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ExportTraceServiceResponse {
    #[prost(message, optional, tag = "1")]
    pub partial_success: Option<ExportTracePartialSuccess>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ExportTracePartialSuccess {
    #[prost(int64, tag = "1")]
    pub rejected_spans: i64,
    #[prost(string, tag = "2")]
    pub error_message: String,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ResourceSpans {
    #[prost(message, optional, tag = "1")]
    pub resource: Option<Resource>,
    #[prost(message, repeated, tag = "2")]
    pub scope_spans: Vec<ScopeSpans>,
    #[prost(string, tag = "3")]
    pub schema_url: String,
    /// Pre-1.0 OTLP name for `scope_spans`; migrated on ingest.
    #[prost(message, repeated, tag = "1000")]
    pub deprecated_scope_spans: Vec<ScopeSpans>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Resource {
    #[prost(message, repeated, tag = "1")]
    pub attributes: Vec<KeyValue>,
    #[prost(uint32, tag = "2")]
    pub dropped_attributes_count: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ScopeSpans {
    #[prost(message, optional, tag = "1")]
    pub scope: Option<InstrumentationScope>,
    #[prost(message, repeated, tag = "2")]
    pub spans: Vec<Span>,
    #[prost(string, tag = "3")]
    pub schema_url: String,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct InstrumentationScope {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub version: String,
    #[prost(message, repeated, tag = "3")]
    pub attributes: Vec<KeyValue>,
    #[prost(uint32, tag = "4")]
    pub dropped_attributes_count: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Span {
    #[prost(bytes = "vec", tag = "1")]
    pub trace_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub span_id: Vec<u8>,
    #[prost(string, tag = "3")]
    pub trace_state: String,
    #[prost(bytes = "vec", tag = "4")]
    pub parent_span_id: Vec<u8>,
    #[prost(string, tag = "5")]
    pub name: String,
    #[prost(enumeration = "SpanKind", tag = "6")]
    pub kind: i32,
    #[prost(fixed64, tag = "7")]
    pub start_time_unix_nano: u64,
    #[prost(fixed64, tag = "8")]
    pub end_time_unix_nano: u64,
    #[prost(message, repeated, tag = "9")]
    pub attributes: Vec<KeyValue>,
    #[prost(uint32, tag = "10")]
    pub dropped_attributes_count: u32,
    #[prost(message, repeated, tag = "11")]
    pub events: Vec<Event>,
    #[prost(uint32, tag = "12")]
    pub dropped_events_count: u32,
    #[prost(message, repeated, tag = "13")]
    pub links: Vec<Link>,
    #[prost(uint32, tag = "14")]
    pub dropped_links_count: u32,
    #[prost(message, optional, tag = "15")]
    pub status: Option<Status>,
    #[prost(fixed32, tag = "16")]
    pub flags: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Event {
    #[prost(fixed64, tag = "1")]
    pub time_unix_nano: u64,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, repeated, tag = "3")]
    pub attributes: Vec<KeyValue>,
    #[prost(uint32, tag = "4")]
    pub dropped_attributes_count: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Link {
    #[prost(bytes = "vec", tag = "1")]
    pub trace_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub span_id: Vec<u8>,
    #[prost(string, tag = "3")]
    pub trace_state: String,
    #[prost(message, repeated, tag = "4")]
    pub attributes: Vec<KeyValue>,
    #[prost(uint32, tag = "5")]
    pub dropped_attributes_count: u32,
    #[prost(fixed32, tag = "6")]
    pub flags: u32,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct KeyValue {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(message, optional, tag = "2")]
    pub value: Option<AnyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct AnyValue {
    #[prost(oneof = "any_value::Value", tags = "1, 2, 3, 4, 5, 6, 7")]
    pub value: Option<any_value::Value>,
}

pub(crate) mod any_value {
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Value {
        #[prost(string, tag = "1")]
        String(String),
        #[prost(bool, tag = "2")]
        Bool(bool),
        #[prost(int64, tag = "3")]
        Int(i64),
        #[prost(double, tag = "4")]
        Double(f64),
        #[prost(message, tag = "5")]
        Array(super::ArrayValue),
        #[prost(message, tag = "6")]
        Kvlist(super::KeyValueList),
        #[prost(bytes = "vec", tag = "7")]
        Bytes(Vec<u8>),
    }
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct ArrayValue {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<AnyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct KeyValueList {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<KeyValue>,
}

#[derive(Clone, PartialEq, Message)]
pub(crate) struct Status {
    #[prost(string, tag = "2")]
    pub message: String,
    #[prost(enumeration = "StatusCode", tag = "3")]
    pub code: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub(crate) enum SpanKind {
    Unspecified = 0,
    Internal = 1,
    Server = 2,
    Client = 3,
    Producer = 4,
    Consumer = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub(crate) enum StatusCode {
    Unset = 0,
    Ok = 1,
    Error = 2,
}

impl Span {
    pub(crate) fn status_code(&self) -> i32 {
        self.status.as_ref().map_or(0, |s| s.code)
    }
}

impl ExportTraceServiceRequest {
    pub(crate) fn span_count(&self) -> usize {
        self.resource_spans
            .iter()
            .flat_map(|rs| &rs.scope_spans)
            .map(|ss| ss.spans.len())
            .sum()
    }

    /// Mirror pdata's post-decode normalisation: pdata stores resource, scope
    /// and status as value fields, so re-encoding a batch always writes them,
    /// even when the sender omitted them. Keep the retained raw batch
    /// byte-compatible with the Go implementation.
    pub(crate) fn fill_defaults(&mut self) {
        for rs in &mut self.resource_spans {
            if rs.resource.is_none() {
                rs.resource = Some(Resource::default());
            }
            if rs.scope_spans.is_empty() {
                rs.scope_spans = std::mem::take(&mut rs.deprecated_scope_spans);
            }
            rs.deprecated_scope_spans.clear();
            for ss in &mut rs.scope_spans {
                if ss.scope.is_none() {
                    ss.scope = Some(InstrumentationScope::default());
                }
                for sp in &mut ss.spans {
                    fill_key_values(&mut sp.attributes);
                    for ev in &mut sp.events {
                        fill_key_values(&mut ev.attributes);
                    }
                    for link in &mut sp.links {
                        fill_key_values(&mut link.attributes);
                    }
                    if sp.status.is_none() {
                        sp.status = Some(Status::default());
                    }
                }
            }
        }
    }

    /// pdata rejects IDs of the wrong length at decode time; so does OTLP.
    pub(crate) fn validate(&self) -> Result<()> {
        for rs in &self.resource_spans {
            for ss in &rs.scope_spans {
                for sp in &ss.spans {
                    check_id(&sp.trace_id, 16, "TraceID")?;
                    check_id(&sp.span_id, 8, "SpanID")?;
                    check_id(&sp.parent_span_id, 8, "SpanID")?;
                    for link in &sp.links {
                        check_id(&link.trace_id, 16, "TraceID")?;
                        check_id(&link.span_id, 8, "SpanID")?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn fill_key_values(attrs: &mut [KeyValue]) {
    for kv in attrs {
        if kv.value.is_none() {
            kv.value = Some(AnyValue::default());
        }
    }
}

fn check_id(id: &[u8], len: usize, name: &str) -> Result<()> {
    if id.is_empty() || id.len() == len {
        return Ok(());
    }
    bail!("unmarshal: invalid {name} length")
}

pub(crate) fn decode(bytes: &[u8]) -> Result<ExportTraceServiceRequest> {
    let mut request =
        ExportTraceServiceRequest::decode(bytes).context("decode OTLP protobuf payload")?;
    request.fill_defaults();
    request.validate()?;
    Ok(request)
}

/// Serialise and gzip a batch for the raw-batch column, the way the Go sink
/// does before normalizing: keeping the decoding input byte-for-byte lets a
/// later `renormalize` replay it.
pub(crate) fn compress(request: &ExportTraceServiceRequest) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&request.encode_to_vec())
        .context("gzip raw OTLP batch")?;
    encoder.finish().context("gzip raw OTLP batch")
}

pub(crate) fn decompress(raw: &[u8]) -> Result<ExportTraceServiceRequest> {
    let mut payload = Vec::new();
    GzDecoder::new(raw)
        .read_to_end(&mut payload)
        .context("gunzip raw OTLP batch")?;
    decode(&payload)
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    out
}

pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd-length hex id {s:?}"));
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = hex_val(pair[0]).ok_or_else(|| format!("invalid hex id {s:?}"))?;
        let lo = hex_val(pair[1]).ok_or_else(|| format!("invalid hex id {s:?}"))?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn is_empty_id(id: &[u8]) -> bool {
    id.iter().all(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(hex_encode(&bytes), "000fa5ff");
        assert_eq!(hex_decode("000fa5ff").unwrap(), bytes);
        assert_eq!(hex_decode("000FA5FF").unwrap(), bytes);
    }

    #[test]
    fn hex_rejects_bad_input() {
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn empty_id_is_all_zero() {
        assert!(is_empty_id(&[]));
        assert!(is_empty_id(&[0; 16]));
        assert!(!is_empty_id(&[0, 1]));
    }
}
