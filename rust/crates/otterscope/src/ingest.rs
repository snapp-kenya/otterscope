//! OTLP/HTTP ingestion and dialect normalization (port of `internal/ingest`).
//! OTel types stop here: everything below sees [`crate::model`].

pub const DEFAULT_MAX_SPANS_PER_BATCH: i64 = 10_000;
pub const DEFAULT_MAX_TRACE_IDS_PER_BATCH: i64 = 1_000;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub rate_per_sec: f64,
    pub burst: f64,
    pub max_spans: i64,
    pub max_trace_ids: i64,
}
