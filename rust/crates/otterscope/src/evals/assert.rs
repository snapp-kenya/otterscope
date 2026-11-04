use anyhow::bail;
use regex::Regex;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::model::{InvalidEnumValue, Run, Step};

/// The deterministic and judge assertion kinds. The database CHECK constraint
/// and this enum must agree; the store decodes rows through [`AssertionKind`]
/// so an invalid value cannot round-trip into an [`Assertion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionKind {
    Contains,
    NotContains,
    Regex,
    IsJson,
    MaxLatencyMs,
    MaxCostUsd,
    LlmJudge,
}

impl AssertionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AssertionKind::Contains => "contains",
            AssertionKind::NotContains => "not_contains",
            AssertionKind::Regex => "regex",
            AssertionKind::IsJson => "is_json",
            AssertionKind::MaxLatencyMs => "max_latency_ms",
            AssertionKind::MaxCostUsd => "max_cost_usd",
            AssertionKind::LlmJudge => "llm_judge",
        }
    }
}

impl TryFrom<&str> for AssertionKind {
    type Error = InvalidEnumValue;

    fn try_from(value: &str) -> std::result::Result<Self, InvalidEnumValue> {
        match value {
            "contains" => Ok(AssertionKind::Contains),
            "not_contains" => Ok(AssertionKind::NotContains),
            "regex" => Ok(AssertionKind::Regex),
            "is_json" => Ok(AssertionKind::IsJson),
            "max_latency_ms" => Ok(AssertionKind::MaxLatencyMs),
            "max_cost_usd" => Ok(AssertionKind::MaxCostUsd),
            "llm_judge" => Ok(AssertionKind::LlmJudge),
            other => Err(InvalidEnumValue::new("assertion type", other)),
        }
    }
}

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

impl Assertion {
    /// Decode the stored/wire spelling into the typed kind.
    pub fn kind(&self) -> std::result::Result<AssertionKind, InvalidEnumValue> {
        AssertionKind::try_from(self.r#type.as_str())
    }
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
    let kind = match a.kind() {
        Ok(kind) => kind,
        Err(_) => bail!("unknown assertion type {:?}", a.r#type),
    };
    match kind {
        AssertionKind::Contains | AssertionKind::NotContains => {
            if a.config.is_empty() {
                bail!("{} needs a non-empty config string", a.r#type);
            }
        }
        AssertionKind::Regex => {
            if let Err(e) = Regex::new(&a.config) {
                bail!("bad regex: {e}");
            }
        }
        AssertionKind::IsJson => {}
        AssertionKind::MaxLatencyMs | AssertionKind::MaxCostUsd => {
            let n: f64 = serde_json::from_str(&a.config).unwrap_or(0.0);
            if n <= 0.0 {
                bail!("{} needs a positive number config", a.r#type);
            }
        }
        AssertionKind::LlmJudge => {
            super::judge::parse_judge_config(&a.config)?;
        }
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
    let Ok(kind) = a.kind() else {
        res.detail = format!("unknown type {:?}", a.r#type);
        return res;
    };
    match kind {
        AssertionKind::Contains => {
            let out = final_output(steps);
            res.pass = out.contains(&a.config);
            if !res.pass {
                res.detail = format!("output does not contain {:?}", a.config);
            }
        }
        AssertionKind::NotContains => {
            let out = final_output(steps);
            res.pass = !out.contains(&a.config);
            if !res.pass {
                res.detail = format!("output contains forbidden {:?}", a.config);
            }
        }
        AssertionKind::Regex => {
            let Ok(re) = Regex::new(&a.config) else {
                res.detail = "invalid regex".into();
                return res;
            };
            res.pass = re.is_match(&final_output(steps));
            if !res.pass {
                res.detail = format!("output does not match /{}/", a.config);
            }
        }
        AssertionKind::IsJson => {
            let out = final_output(steps);
            let trimmed = out.trim();
            res.pass = !trimmed.is_empty() && valid_json(trimmed);
            if !res.pass {
                res.detail = "output is not valid JSON".into();
            }
        }
        AssertionKind::MaxLatencyMs => {
            let max_ms: f64 = serde_json::from_str(&a.config).unwrap_or(0.0);
            let end = run.end.unwrap_or(run.start);
            let got = (end - run.start).num_milliseconds() as f64;
            res.pass = got <= max_ms;
            if !res.pass {
                res.detail = format!("run took {got:.0}ms > {max_ms:.0}ms");
            }
        }
        AssertionKind::MaxCostUsd => {
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
        AssertionKind::LlmJudge => {
            res.detail = "llm_judge is evaluated by the judge endpoint".into();
        }
    }
    res
}
