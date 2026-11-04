use anyhow::bail;
use regex::Regex;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::model::{Run, Step};

/// One per-project check applied to completed runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    pub id: i64,
    pub project: String,
    pub name: String,
    /// `contains` | `not_contains` | `regex` | `is_json` | `max_latency_ms` |
    /// `max_cost_usd` | `llm_judge`.
    #[serde(rename = "type")]
    pub r#type: String,
    /// Meaning depends on `r#type`.
    pub config: String,
    pub enabled: bool,
}

/// Result of one assertion against one run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Result {
    pub assertion_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub pass: bool,
    pub detail: String,
}

impl Result {
    pub(crate) fn for_assertion(a: &Assertion) -> Self {
        Self {
            assertion_id: a.id,
            name: a.name.clone(),
            r#type: a.r#type.clone(),
            pass: false,
            detail: String::new(),
        }
    }
}

/// Checks that an assertion's type/config combination is usable.
pub trait Validate {
    fn validate(&self) -> anyhow::Result<()>;
}

impl Validate for Assertion {
    fn validate(&self) -> anyhow::Result<()> {
        validate(self)
    }
}

pub fn validate(a: &Assertion) -> anyhow::Result<()> {
    match a.r#type.as_str() {
        "contains" | "not_contains" => {
            if a.config.is_empty() {
                bail!("{} needs a non-empty config string", a.r#type);
            }
        }
        "regex" => {
            if let Err(e) = Regex::new(&a.config) {
                bail!("bad regex: {e}");
            }
        }
        "is_json" => {}
        "max_latency_ms" | "max_cost_usd" => {
            let n: f64 = serde_json::from_str(&a.config).unwrap_or(0.0);
            if n <= 0.0 {
                bail!("{} needs a positive number config", a.r#type);
            }
        }
        "llm_judge" => {
            super::judge::parse_judge_config(&a.config)?;
        }
        other => bail!("unknown assertion type {other:?}"),
    }
    Ok(())
}

/// The text an output assertion targets: the last LLM step's output messages,
/// concatenated.
pub fn final_output(steps: &[Step]) -> String {
    for step in steps.iter().rev() {
        let Some(llm) = &step.llm else { continue };
        if llm.output_messages.is_empty() {
            continue;
        }
        let mut out = String::new();
        for message in &llm.output_messages {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&message.content);
        }
        return out;
    }
    String::new()
}

/// Syntax-only JSON validation, like Go's `json.Valid`: no value is built, so
/// huge numbers and deeply nested documents remain valid.
fn valid_json(text: &str) -> bool {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    IgnoredAny::deserialize(&mut deserializer).is_ok() && deserializer.end().is_ok()
}

/// Runs one deterministic assertion against a completed run.
///
/// `llm_judge` assertions are not evaluated here: they need network access and
/// a server-configured endpoint, so callers use [`super::judge`] instead.
pub fn evaluate(a: &Assertion, run: &Run, steps: &[Step]) -> Result {
    let mut res = Result::for_assertion(a);
    match a.r#type.as_str() {
        "contains" => {
            let out = final_output(steps);
            res.pass = out.contains(&a.config);
            if !res.pass {
                res.detail = format!("output does not contain {:?}", a.config);
            }
        }
        "not_contains" => {
            let out = final_output(steps);
            res.pass = !out.contains(&a.config);
            if !res.pass {
                res.detail = format!("output contains forbidden {:?}", a.config);
            }
        }
        "regex" => {
            let Ok(re) = Regex::new(&a.config) else {
                res.detail = "invalid regex".into();
                return res;
            };
            res.pass = re.is_match(&final_output(steps));
            if !res.pass {
                res.detail = format!("output does not match /{}/", a.config);
            }
        }
        "is_json" => {
            let out = final_output(steps);
            let trimmed = out.trim();
            res.pass = !trimmed.is_empty() && valid_json(trimmed);
            if !res.pass {
                res.detail = "output is not valid JSON".into();
            }
        }
        "max_latency_ms" => {
            let max_ms: f64 = serde_json::from_str(&a.config).unwrap_or(0.0);
            let end = run.end.unwrap_or(run.start);
            let got = (end - run.start).num_milliseconds() as f64;
            res.pass = got <= max_ms;
            if !res.pass {
                res.detail = format!("run took {got:.0}ms > {max_ms:.0}ms");
            }
        }
        "max_cost_usd" => {
            let max_usd: f64 = serde_json::from_str(&a.config).unwrap_or(0.0);
            let Some(cost) = run.cost_usd else {
                res.detail = "run cost unknown (unpriced model)".into();
                return res;
            };
            res.pass = cost <= max_usd;
            if !res.pass {
                res.detail = format!("run cost ${cost:.4} > ${max_usd:.4}");
            }
        }
        other => {
            res.detail = format!("unknown type {other:?}");
        }
    }
    res
}
