//! Effective `serve` configuration: defaults, then JSON file, then
//! `OTTERSCOPE_*` environment variables, with explicit flags overriding the
//! result (flags > env > file > defaults — the same precedence the Go version
//! documents and scripts rely on).

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::basepath;

/// Default cap on spans carried by one OTLP request (mirrors
/// `ingest.DefaultMaxSpansPerBatch`).
pub const DEFAULT_MAX_SPANS_PER_BATCH: i64 = 10_000;
/// Default cap on distinct trace ids one OTLP request may touch.
pub const DEFAULT_MAX_TRACE_IDS_PER_BATCH: i64 = 1_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub db: String,
    pub listen: String,
    pub otlp: String,
    pub pricing: String,
    pub judge_url: String,
    /// Allow-list of models an `llm_judge` assertion may name. Empty means
    /// none are allowed: assertions are writable through the API, so a caller
    /// must not be able to pick an arbitrary (and arbitrarily priced) model.
    pub judge_models: Vec<String>,
    pub retention: Duration,
    pub alert_interval: Duration,
    pub read_auth: bool,
    pub ingest_rate: f64,
    pub ingest_burst: f64,
    pub max_spans_per_batch: i64,
    pub max_trace_ids_per_batch: i64,
    pub base_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db: "otterscope.db".into(),
            listen: "127.0.0.1:8317".into(),
            otlp: "127.0.0.1:4318".into(),
            pricing: String::new(),
            judge_url: "https://api.openai.com/v1".into(),
            judge_models: Vec::new(),
            retention: Duration::ZERO,
            alert_interval: Duration::from_secs(60),
            read_auth: false,
            ingest_rate: 50.0,
            ingest_burst: 100.0,
            max_spans_per_batch: DEFAULT_MAX_SPANS_PER_BATCH,
            max_trace_ids_per_batch: DEFAULT_MAX_TRACE_IDS_PER_BATCH,
            base_path: String::new(),
        }
    }
}

/// JSON schema of the config file. `Option` distinguishes "absent" (keep the
/// current value) from an explicit zero, exactly like the Go pointer fields.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FileConfig {
    db: Option<String>,
    listen: Option<String>,
    otlp: Option<String>,
    pricing: Option<String>,
    judge_url: Option<String>,
    judge_models: Option<Vec<String>>,
    retention: Option<String>,
    alert_interval: Option<String>,
    read_auth: Option<bool>,
    ingest_rate: Option<f64>,
    ingest_burst: Option<f64>,
    max_spans_per_batch: Option<i64>,
    max_trace_ids_per_batch: Option<i64>,
    base_path: Option<String>,
}

impl Config {
    /// Load a config file at `path` (if non-empty) and then apply `OTTERSCOPE_*`
    /// environment variables via `getenv`.
    pub fn load(path: &str, getenv: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut c = Config::default();
        if !path.is_empty() {
            c.apply_file(Path::new(path))?;
        }
        c.apply_env(getenv)?;
        c.base_path = basepath::normalize(&c.base_path)?;
        Ok(c)
    }

    fn apply_file(&mut self, path: &Path) -> Result<(), String> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("read config {}: {e}", path.display()))?;
        let f: FileConfig = serde_json::from_str(&data)
            .map_err(|e| format!("parse config {}: {e}", path.display()))?;

        if let Some(v) = f.db {
            self.db = v;
        }
        if let Some(v) = f.listen {
            self.listen = v;
        }
        if let Some(v) = f.otlp {
            self.otlp = v;
        }
        if let Some(v) = f.pricing {
            self.pricing = v;
        }
        if let Some(v) = f.judge_url {
            self.judge_url = v;
        }
        if let Some(v) = f.base_path {
            self.base_path = v;
        }
        if let Some(v) = f.read_auth {
            self.read_auth = v;
        }
        if let Some(v) = f.ingest_rate {
            self.ingest_rate = v;
        }
        if let Some(v) = f.ingest_burst {
            self.ingest_burst = v;
        }
        if let Some(v) = f.max_spans_per_batch {
            self.max_spans_per_batch = v;
        }
        if let Some(v) = f.max_trace_ids_per_batch {
            self.max_trace_ids_per_batch = v;
        }
        if let Some(v) = f.retention {
            self.retention = parse_duration(&v).map_err(|e| format!("config retention: {e}"))?;
        }
        if let Some(v) = f.judge_models {
            self.judge_models = clean_models(v);
        }
        if let Some(v) = f.alert_interval {
            self.alert_interval =
                parse_duration(&v).map_err(|e| format!("config alertInterval: {e}"))?;
        }
        Ok(())
    }

    fn apply_env(&mut self, getenv: impl Fn(&str) -> Option<String>) -> Result<(), String> {
        let s = |k: &str| getenv(k).filter(|v| !v.is_empty());

        if let Some(v) = s("OTTERSCOPE_DB") {
            self.db = v;
        }
        if let Some(v) = s("OTTERSCOPE_LISTEN") {
            self.listen = v;
        }
        if let Some(v) = s("OTTERSCOPE_OTLP") {
            self.otlp = v;
        }
        if let Some(v) = s("OTTERSCOPE_PRICING") {
            self.pricing = v;
        }
        if let Some(v) = s("OTTERSCOPE_JUDGE_URL") {
            self.judge_url = v;
        }
        if let Some(v) = s("OTTERSCOPE_BASE_PATH") {
            self.base_path = v;
        }
        if let Some(v) = s("OTTERSCOPE_JUDGE_MODELS") {
            self.judge_models = parse_model_list(&v);
        }
        if let Some(v) = s("OTTERSCOPE_READ_AUTH") {
            self.read_auth = parse_bool(&v).map_err(|e| format!("OTTERSCOPE_READ_AUTH: {e}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_INGEST_RATE") {
            self.ingest_rate = v
                .parse()
                .map_err(|_| format!("OTTERSCOPE_INGEST_RATE: invalid value {v:?}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_INGEST_BURST") {
            self.ingest_burst = v
                .parse()
                .map_err(|_| format!("OTTERSCOPE_INGEST_BURST: invalid value {v:?}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_MAX_SPANS_PER_BATCH") {
            self.max_spans_per_batch = v
                .parse()
                .map_err(|_| format!("OTTERSCOPE_MAX_SPANS_PER_BATCH: invalid value {v:?}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_MAX_TRACE_IDS_PER_BATCH") {
            self.max_trace_ids_per_batch = v
                .parse()
                .map_err(|_| format!("OTTERSCOPE_MAX_TRACE_IDS_PER_BATCH: invalid value {v:?}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_RETENTION") {
            self.retention =
                parse_duration(&v).map_err(|e| format!("OTTERSCOPE_RETENTION: {e}"))?;
        }
        if let Some(v) = s("OTTERSCOPE_ALERT_INTERVAL") {
            self.alert_interval =
                parse_duration(&v).map_err(|e| format!("OTTERSCOPE_ALERT_INTERVAL: {e}"))?;
        }
        Ok(())
    }
}

/// Parse a comma-separated judge-model allow-list, trimming blanks so `"a, b,"`
/// and `["a","b"]` mean the same and a blank element cannot behave like a
/// wildcard.
pub fn parse_model_list(s: &str) -> Vec<String> {
    clean_models(s.split(',').map(str::to_string).collect())
}

fn clean_models(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("invalid bool {v:?}")),
    }
}

/// Parse a Go `time.ParseDuration` string (`"720h"`, `"1m30s"`, `"1.5h"`).
/// Kept compatible because the same spelling appears in flags, config files and
/// env vars, and operators have those in scripts and Compose files.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err("time: invalid duration \"\"".into());
    }
    if raw == "0" {
        return Ok(Duration::ZERO);
    }
    let (neg, rest) = match raw.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };
    if rest.is_empty() {
        return Err(format!("time: invalid duration {s:?}"));
    }

    let mut total = 0f64;
    let mut num = String::new();
    let mut chars = rest.char_indices().peekable();
    let mut saw_unit = false;
    while let Some((i, ch)) = chars.next() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
            continue;
        }
        // A unit is one or more alphabetic/µ chars; collect the full unit.
        let start = i;
        let mut end = i + ch.len_utf8();
        while let Some((j, c2)) = chars.peek() {
            if c2.is_alphabetic() || *c2 == 'µ' || *c2 == 'μ' {
                end = j + c2.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let unit = &rest[start..end];
        let value: f64 = num
            .parse()
            .map_err(|_| format!("time: invalid duration {s:?}"))?;
        num.clear();
        let scale = match unit {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return Err(format!("time: unknown unit {unit:?} in duration {s:?}")),
        };
        total += value * scale;
        saw_unit = true;
    }
    if !num.is_empty() || !saw_unit {
        return Err(format!("time: missing unit in duration {s:?}"));
    }
    let secs = if neg { -total } else { total };
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_match_go_spellings() {
        assert_eq!(
            parse_duration("720h").unwrap(),
            Duration::from_secs(2_592_000)
        );
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1.5h").unwrap(), Duration::from_secs(5400));
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("10x").is_err());
    }

    #[test]
    fn env_overrides_defaults_and_blank_is_ignored() {
        let c = Config::load("", |k| match k {
            "OTTERSCOPE_READ_AUTH" => Some("true".into()),
            "OTTERSCOPE_LISTEN" => Some(":9999".into()),
            "OTTERSCOPE_DB" => Some(String::new()),
            _ => None,
        })
        .unwrap();
        assert!(c.read_auth);
        assert_eq!(c.listen, ":9999");
        assert_eq!(c.db, "otterscope.db");
    }

    #[test]
    fn base_path_is_canonicalised() {
        let c = Config::load("", |k| {
            (k == "OTTERSCOPE_BASE_PATH").then(|| "otterscope/".to_string())
        })
        .unwrap();
        assert_eq!(c.base_path, "/otterscope");
    }
}
