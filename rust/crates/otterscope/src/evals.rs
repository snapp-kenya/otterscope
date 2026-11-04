//! Assertions and LLM-as-judge evals (port of `internal/evals`).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Assertion {
    pub id: i64,
    pub project: String,
    pub name: String,
    pub r#type: String,
    pub config: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Result {
    pub assertion_id: i64,
    pub name: String,
    pub r#type: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct Judge;
