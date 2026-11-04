//! The domain: agent runs and their steps.
//!
//! Everything downstream of [`crate::ingest`] depends on these types and never
//! on OTel types — the boundary ADR-0002 draws, preserved verbatim in the port.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Ok,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Ok => "ok",
            Status::Error => "error",
        }
    }

    /// Higher wins when a run's status is derived from its steps: a delivered
    /// root makes the run finished, and any error outranks a clean finish.
    pub fn rank(self) -> u8 {
        match self {
            Status::Running => 0,
            Status::Ok => 1,
            Status::Error => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Agent,
    Llm,
    Tool,
    Generic,
}

impl StepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Agent => "agent",
            StepKind::Llm => "llm",
            StepKind::Tool => "tool",
            StepKind::Generic => "generic",
        }
    }
}

/// One end-to-end agent execution — a trace, in OTel terms. Aggregate fields
/// are derived from the run's steps at write time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub project: String,
    pub service: String,
    pub agent_name: String,
    pub status: Status,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub llm_calls: i64,
    pub tool_calls: i64,
    /// Distinct request models, comma-joined.
    pub models: String,
    /// Distinct prompt identities, comma-joined.
    pub prompts: String,
    /// Sum of known step costs; `None` when none is known.
    pub cost_usd: Option<f64>,
    /// True when some LLM steps had no known price.
    pub cost_partial: bool,
    /// First step error encountered.
    pub error: String,
}

/// One operation within a run — a span, in OTel terms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub project: String,
    pub run_id: String,
    /// Parent span id; empty for the root step.
    pub parent_id: String,
    pub kind: StepKind,
    pub name: String,
    pub service: String,
    pub agent_name: String,
    pub status: Status,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmCall {
    pub provider: String,
    pub request_model: String,
    pub response_model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Subsets of the totals above; needed for accurate cost math.
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub reasoning_tokens: i64,
    pub input_messages: Vec<Message>,
    pub output_messages: Vec<Message>,
    /// Set at ingest from the pricing table; `None` = unknown model.
    pub cost_usd: Option<f64>,
    /// Prompt identity (name/version) for A/B tracking; empty when untagged.
    pub prompt: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub call_id: String,
    pub arguments: String,
    pub result: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_outranks_ok_and_running() {
        assert!(Status::Error.rank() > Status::Ok.rank());
        assert!(Status::Ok.rank() > Status::Running.rank());
    }
}
