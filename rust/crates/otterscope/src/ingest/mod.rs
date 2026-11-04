//! OTLP/HTTP ingestion and dialect normalization (ported from the original Go
//! implementation).
//!
//! This is the only module that knows OpenTelemetry exists: the receiver
//! decodes OTLP (protobuf or JSON), normalizes every span to
//! [`crate::model::Step`], and hands the sink plain domain types plus the raw
//! batch it should retain. Downstream modules — server, store, evals — never
//! import OTel types (ADR-0002).
//!
//! Dialects covered:
//! - OTel GenAI, both the stable `gen_ai.*` names and the
//!   `gen_ai_latest_experimental` additions
//! - OpenInference (Arize): `openinference.span.kind`
//! - Vercel AI SDK: `ai.*` span names and attributes
//! - Claude Agent SDK / Claude Code: `claude_code.*` spans

mod attributes;
mod dialect_claudecode;
mod dialect_openinference;
mod dialect_vercel;
mod json;
mod messages;
mod normalize;
mod proto;
mod ratelimit;
mod receiver;

#[cfg(test)]
mod testutil;

use std::sync::Arc;

use crate::model::Step;

pub use normalize::renormalize_batch;
pub use receiver::{Receiver, router};

/// The most spans one request may carry.
pub const DEFAULT_MAX_SPANS_PER_BATCH: i64 = 10_000;
/// The most distinct trace IDs (runs) one request may touch, which bounds the
/// run re-derivation work.
pub const DEFAULT_MAX_TRACE_IDS_PER_BATCH: i64 = 1_000;

/// Configures the OTLP receiver.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Mounts the receiver under a path prefix (`""` = listener root).
    pub base_path: String,
    /// Per-project ingest rate limit; 0 means unlimited.
    pub rate_per_sec: f64,
    /// Token-bucket size; 0 means 2x the rate.
    pub burst: f64,
    /// Rejects requests that present no ingest key. Set it whenever read auth
    /// is on: otherwise an unauthenticated sender still lands in the
    /// "default" project and can rewrite its steps.
    pub require_key: bool,
    /// Cap on spans per request; 0 means the package default.
    pub max_spans: i64,
    /// Cap on distinct trace IDs per request; 0 means the package default.
    pub max_trace_ids: i64,
}

impl Options {
    pub(crate) fn max_spans(&self) -> usize {
        let cap = if self.max_spans > 0 {
            self.max_spans
        } else {
            DEFAULT_MAX_SPANS_PER_BATCH
        };
        usize::try_from(cap).unwrap_or(usize::MAX)
    }

    pub(crate) fn max_trace_ids(&self) -> usize {
        let cap = if self.max_trace_ids > 0 {
            self.max_trace_ids
        } else {
            DEFAULT_MAX_TRACE_IDS_PER_BATCH
        };
        usize::try_from(cap).unwrap_or(usize::MAX)
    }
}

/// What a sink did with one batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestResult {
    /// Spans not written because their `(project, span ID)` already exists for
    /// a different resource service. Reported to the client via
    /// `partialSuccess`, never silently.
    pub rejected_spans: i64,
    /// Human-readable explanation of the rejection, for the client's log.
    pub message: String,
}

impl IngestResult {
    /// Build the result the store reports when it refused spans whose span ID
    /// belongs to another service.
    pub fn with_rejections(rejected_spans: i64) -> Self {
        Self {
            rejected_spans,
            message: if rejected_spans > 0 {
                format!(
                    "{rejected_spans} span(s) rejected: an existing step with the same span id belongs to a different service"
                )
            } else {
                String::new()
            },
        }
    }
}

/// Receives every decoded batch, tagged with the project the caller's ingest
/// key resolved to. Implementations must be safe for concurrent use; the
/// receiver calls `consume` on a blocking worker thread.
///
/// `raw` is the gzip-compressed protobuf encoding of the batch, for the
/// store's raw-batch column (so a later renormalization can replay it);
/// `steps` are already normalized, with `project` stamped on each step.
pub trait Sink: Send + Sync {
    fn consume(&self, project: &str, raw: &[u8], steps: &[Step]) -> anyhow::Result<IngestResult>;
}

/// Maps a Bearer ingest key (`""` = none supplied) to a project name; `None`
/// rejects the request.
pub type KeyResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
