//! Assertions and LLM-as-judge evals (ported from the original Go
//! implementation).
//!
//! Results live on the run (ADR-0003: evals fused into the trace store, no
//! second product). Deterministic assertion types are evaluated locally;
//! `llm_judge` delegates to the server-configured OpenAI-compatible endpoint.
//!
//! The endpoint, API key and model allow-list are deliberately *not* part of
//! an assertion: assertions are writable through the unauthenticated
//! single-user API, so letting one name a URL or secret would allow SSRF and
//! secret exfiltration (audit finding #48).

mod assert;
mod judge;

#[cfg(test)]
mod tests;

pub use assert::{Assertion, Result, Validate, evaluate, final_output, validate};
pub use judge::{Endpoint, JudgeConfig, judge, parse_judge_config};
