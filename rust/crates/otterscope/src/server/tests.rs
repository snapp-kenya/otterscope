//! In-process tests for the HTTP layer.
//!
//! The backend is an in-memory fake so guard, auth, base-path and wire-shape
//! behaviour is exercised without the SQLite store.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use tokio::sync::broadcast;
use tower::ServiceExt;

use super::backend::{
    AlertRecord, ApiBackend, AuditRecord, BackendError, BackendResult, CreatedTokenRecord,
    EvaluateOptions, EvaluateReport, NewAlert, ShareRecord, StatsRecord, TokenRecord, ViewRecord,
};
use super::stream::Hub;
use super::{Options, build, build_with_ingest};
use crate::evals::{Assertion, Result as EvalResult};
use crate::model::{LlmCall, Message, Run, Status, Step, StepKind};
use crate::store::{Filter, Metrics, Scope};

const BASE: &str = "/otterscope";

// ------------------------------------------------------------------ the fake

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<FakeState>>);

#[derive(Default)]
struct FakeState {
    runs: Vec<(Run, Vec<Step>)>,
    assertions: Vec<Assertion>,
    results: BTreeMap<(String, String), Vec<EvalResult>>,
    views: Vec<ViewRecord>,
    next_view_id: i64,
    next_assertion_id: i64,
    tokens: Vec<(String, TokenRecord)>,
    next_token: i64,
    shares: Vec<(String, String, String)>,
    alerts: Vec<AlertRecord>,
    next_alert_id: i64,
    audit: Vec<AuditRecord>,
    judge_models: Vec<String>,
    max_evaluate_runs: Option<i64>,
    max_judge_calls: Option<i64>,
    broken: bool,
    /// Thread that served each backend call, so tests can prove the blocking
    /// pool, not the async worker, runs the store work.
    threads: Vec<std::thread::ThreadId>,
}

impl Fake {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(FakeState::default())))
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.0.lock().expect("fake backend")
    }
}

fn broken() -> BackendError {
    BackendError::other(anyhow::anyhow!("store unavailable"))
}

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0)
        .single()
        .expect("timestamp")
}

fn within(run: &Run, since: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> bool {
    since.is_none_or(|since| run.start >= since) && until.is_none_or(|until| run.start <= until)
}

fn matches_filter(run: &Run, steps: &[Step], filter: &Filter) -> bool {
    if !filter.project.is_empty() && run.project != filter.project {
        return false;
    }
    if !filter.status.is_empty() && run.status.as_str() != filter.status {
        return false;
    }
    if !filter.service.is_empty() && !run.service.starts_with(&filter.service) {
        return false;
    }
    if !filter.model.is_empty() && !run.models.contains(&filter.model) {
        return false;
    }
    if !filter.prompt.is_empty() && !run.prompts.contains(&filter.prompt) {
        return false;
    }
    if !filter.query.is_empty() {
        let needle = filter.query.to_lowercase();
        let haystack = steps
            .iter()
            .map(|step| {
                let mut text = format!("{} {}", step.name, step.error);
                if let Some(llm) = &step.llm {
                    text.push_str(&format!(" {} {}", llm.provider, llm.request_model));
                    for message in llm.input_messages.iter().chain(&llm.output_messages) {
                        text.push(' ');
                        text.push_str(&message.content);
                    }
                }
                if let Some(tool) = &step.tool {
                    text.push_str(&format!(
                        " {} {} {}",
                        tool.name, tool.arguments, tool.result
                    ));
                }
                text
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !haystack.to_lowercase().contains(&needle) {
            return false;
        }
    }
    within(run, filter.since, filter.until)
}

fn final_output(steps: &[Step]) -> String {
    for step in steps.iter().rev() {
        let Some(llm) = &step.llm else { continue };
        if llm.output_messages.is_empty() {
            continue;
        }
        return llm
            .output_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

fn evaluate_assertion(assertion: &Assertion, run: &Run, steps: &[Step]) -> EvalResult {
    let output = final_output(steps);
    let (pass, detail) = match assertion.r#type.as_str() {
        "contains" => {
            let pass = output.contains(&assertion.config);
            (
                pass,
                if pass {
                    String::new()
                } else {
                    format!("output does not contain {:?}", assertion.config)
                },
            )
        }
        "not_contains" => {
            let pass = !output.contains(&assertion.config);
            (
                pass,
                if pass {
                    String::new()
                } else {
                    format!("output contains forbidden {:?}", assertion.config)
                },
            )
        }
        "regex" => match regex::Regex::new(&assertion.config) {
            Ok(pattern) => {
                let pass = pattern.is_match(&output);
                (
                    pass,
                    if pass {
                        String::new()
                    } else {
                        format!("output does not match /{}/", assertion.config)
                    },
                )
            }
            Err(_) => (false, "invalid regex".to_string()),
        },
        "is_json" => {
            let trimmed = output.trim();
            let pass = !trimmed.is_empty() && serde_json::from_str::<Value>(trimmed).is_ok();
            (
                pass,
                if pass {
                    String::new()
                } else {
                    "output is not valid JSON".to_string()
                },
            )
        }
        "max_latency_ms" => {
            let max: f64 = assertion.config.parse().unwrap_or(0.0);
            let got = (run.end.unwrap_or(run.start) - run.start).num_milliseconds() as f64;
            let pass = got <= max;
            (
                pass,
                if pass {
                    String::new()
                } else {
                    format!("latency {got}ms exceeds {max}ms")
                },
            )
        }
        "max_cost_usd" => {
            let max: f64 = assertion.config.parse().unwrap_or(0.0);
            match run.cost_usd {
                Some(cost) if cost > max => (false, format!("cost {cost} exceeds {max}")),
                _ => (true, String::new()),
            }
        }
        "llm_judge" => (true, "PASS".to_string()),
        other => (false, format!("unknown assertion type {other:?}")),
    };
    EvalResult {
        assertion_id: assertion.id,
        name: assertion.name.clone(),
        r#type: assertion.r#type.clone(),
        pass,
        detail,
    }
}

fn judge_config(config: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        prompt: String,
        model: String,
        #[serde(default, rename = "sampleRate")]
        sample_rate: f64,
    }
    let parsed: Config = serde_json::from_str(config).map_err(|err| {
        format!("llm_judge config invalid (only prompt, model, sampleRate are allowed): {err}")
    })?;
    if parsed.prompt.is_empty() || parsed.model.is_empty() {
        return Err("llm_judge config needs prompt and model".to_string());
    }
    if !(0.0..=1.0).contains(&parsed.sample_rate) {
        return Err("sampleRate must be within 0..1".to_string());
    }
    Ok(parsed.model)
}

fn validate_assertion(assertion: &Assertion) -> Result<(), String> {
    match assertion.r#type.as_str() {
        "contains" | "not_contains" => {
            if assertion.config.is_empty() {
                return Err(format!(
                    "{} needs a non-empty config string",
                    assertion.r#type
                ));
            }
        }
        "regex" => {
            regex::Regex::new(&assertion.config).map_err(|err| format!("bad regex: {err}"))?;
        }
        "is_json" => {}
        "max_latency_ms" | "max_cost_usd" => match assertion.config.parse::<f64>() {
            Ok(value) if value > 0.0 => {}
            _ => {
                return Err(format!(
                    "{} needs a positive number config",
                    assertion.r#type
                ));
            }
        },
        "llm_judge" => {
            judge_config(&assertion.config)?;
        }
        other => return Err(format!("unknown assertion type {other:?}")),
    }
    Ok(())
}

fn webhook_ip_allowed(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || octets[0] >= 240)
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80)
        }
    }
}

fn validate_webhook(raw: &str) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("webhook URL is empty".to_string());
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return Err("webhook URL must be absolute, e.g. https://hooks.example.com/…".into());
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "webhook URL must use http or https, not {scheme:?}"
        ));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(host) = authority.strip_prefix('[') {
        host.split_once(']').map(|(host, _)| host).unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err("webhook URL has no host".into());
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Err(format!("webhook URL points at a loopback address ({host})"));
    }
    if let Ok(address) = host.parse::<std::net::IpAddr>()
        && !webhook_ip_allowed(&address)
    {
        return Err(format!(
            "webhook URL points at a private address ({address})"
        ));
    }
    Ok(())
}

fn validate_alert(alert: &NewAlert) -> Result<(), String> {
    if alert.name.is_empty() {
        return Err("alert needs a name".into());
    }
    if alert.webhook_url.is_empty() {
        return Err("alert needs a webhook URL".into());
    }
    validate_webhook(&alert.webhook_url)?;
    if alert.window_secs <= 0 {
        return Err("windowSecs must be positive".into());
    }
    if alert.window_secs > crate::alerts::MAX_WINDOW_SECS {
        return Err(format!(
            "windowSecs {} is over the {} second maximum (30 days)",
            alert.window_secs,
            crate::alerts::MAX_WINDOW_SECS
        ));
    }
    if alert.settle_secs < 0 {
        return Err("settleSecs cannot be negative".into());
    }
    if alert.settle_secs > 24 * 60 * 60 {
        return Err("settleSecs is over the 86400 second maximum".into());
    }
    match alert.kind.as_str() {
        "error_rate" | "assertion_fail_rate" => {
            if !(0.0..=1.0).contains(&alert.threshold) {
                return Err(format!("{} threshold is a fraction 0..1", alert.kind));
            }
            if alert.kind == "assertion_fail_rate" && alert.config.is_empty() {
                return Err("assertion_fail_rate needs config = assertion name".into());
            }
        }
        "cost" | "p95_latency" => {
            if alert.threshold <= 0.0 {
                return Err(format!("{} threshold must be positive", alert.kind));
            }
        }
        other => return Err(format!("unknown alert type {other:?}")),
    }
    Ok(())
}

fn redact_webhook(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return "***".into();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return "***".into();
    }
    let path = rest[authority.len()..].split('?').next().unwrap_or("");
    if path.is_empty() || path == "/" {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/***")
    }
}

impl ApiBackend for Fake {
    fn list_runs(&self, filter: &Filter, limit: i64, offset: i64) -> BackendResult<Vec<Run>> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        if state.broken {
            return Err(broken());
        }
        let mut runs: Vec<Run> = state
            .runs
            .iter()
            .filter(|(run, steps)| matches_filter(run, steps, filter))
            .map(|(run, _)| run.clone())
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.start));
        Ok(runs
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect())
    }

    fn get_run(&self, id: &str) -> BackendResult<(Run, Vec<Step>)> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        if state.broken {
            return Err(broken());
        }
        state
            .runs
            .iter()
            .find(|(run, _)| run.id == id)
            .map(|(run, steps)| (run.clone(), steps.clone()))
            .ok_or(BackendError::NotFound)
    }

    fn get_run_in_project(&self, project: &str, id: &str) -> BackendResult<(Run, Vec<Step>)> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        state
            .runs
            .iter()
            .find(|(run, _)| run.id == id && run.project == project)
            .map(|(run, steps)| (run.clone(), steps.clone()))
            .ok_or(BackendError::NotFound)
    }

    fn get_stats(&self, filter: &Filter) -> BackendResult<StatsRecord> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let selected: Vec<&(Run, Vec<Step>)> = state
            .runs
            .iter()
            .filter(|(run, steps)| matches_filter(run, steps, filter))
            .collect();
        let mut durations: Vec<i64> = selected
            .iter()
            .map(|(run, _)| (run.end.unwrap_or(run.start) - run.start).num_milliseconds())
            .collect();
        durations.sort_unstable();
        let percentile = |p: f64| -> i64 {
            if durations.is_empty() {
                return 0;
            }
            let index = ((durations.len() as f64 - 1.0) * p).round() as usize;
            durations[index.min(durations.len() - 1)]
        };
        let runs = selected.len() as i64;
        let errors = selected
            .iter()
            .filter(|(run, _)| run.status == Status::Error)
            .count() as i64;
        let total_cost: f64 = selected.iter().filter_map(|(run, _)| run.cost_usd).sum();
        let any_cost = selected.iter().any(|(run, _)| run.cost_usd.is_some());

        let mut counts: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
        for (run, _) in &selected {
            let key = (run.project.clone(), run.id.clone());
            let Some(results) = state.results.get(&key) else {
                continue;
            };
            for result in results {
                let entry = counts.entry(result.assertion_id).or_insert((0, 0));
                if result.pass {
                    entry.0 += 1;
                }
                entry.1 += 1;
            }
        }
        let assertion_rates = counts
            .into_iter()
            .map(
                |(assertion_id, (passed, total))| super::backend::AssertionRateRecord {
                    assertion_id,
                    name: state
                        .assertions
                        .iter()
                        .find(|assertion| assertion.id == assertion_id)
                        .map(|assertion| assertion.name.clone())
                        .unwrap_or_default(),
                    passed,
                    total,
                },
            )
            .collect();

        Ok(StatsRecord {
            runs,
            errors,
            avg_duration_ms: if runs > 0 {
                durations.iter().sum::<i64>() as f64 / runs as f64
            } else {
                0.0
            },
            p50_duration_ms: percentile(0.5),
            p95_duration_ms: percentile(0.95),
            total_cost_usd: any_cost.then_some(total_cost),
            avg_cost_usd: (any_cost && runs > 0).then_some(total_cost / runs as f64),
            input_tokens: selected.iter().map(|(run, _)| run.input_tokens).sum(),
            output_tokens: selected.iter().map(|(run, _)| run.output_tokens).sum(),
            assertion_rates,
        })
    }

    fn metrics(&self) -> BackendResult<Metrics> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(Metrics {
            runs: state.runs.len() as i64,
            runs_ok: state
                .runs
                .iter()
                .filter(|(run, _)| run.status == Status::Ok)
                .count() as i64,
            runs_error: state
                .runs
                .iter()
                .filter(|(run, _)| run.status == Status::Error)
                .count() as i64,
            runs_running: state
                .runs
                .iter()
                .filter(|(run, _)| run.status == Status::Running)
                .count() as i64,
            steps: state
                .runs
                .iter()
                .map(|(_, steps)| steps.len())
                .sum::<usize>() as i64,
            projects: 1,
            alerts_firing: state.alerts.iter().filter(|alert| alert.firing).count() as i64,
            db_size_bytes: 4096,
        })
    }

    fn list_assertions(&self, project: &str) -> BackendResult<Vec<Assertion>> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(state
            .assertions
            .iter()
            .filter(|assertion| project.is_empty() || assertion.project == project)
            .cloned()
            .collect())
    }

    fn create_assertion(&self, mut assertion: Assertion) -> BackendResult<Assertion> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        if assertion.project.is_empty() {
            assertion.project = "default".into();
        }
        if state
            .assertions
            .iter()
            .any(|other| other.project == assertion.project && other.name == assertion.name)
        {
            return Err(BackendError::Conflict);
        }
        state.next_assertion_id += 1;
        assertion.id = state.next_assertion_id;
        state.assertions.push(assertion.clone());
        Ok(assertion)
    }

    fn delete_assertion(&self, id: i64) -> BackendResult<()> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let before = state.assertions.len();
        state.assertions.retain(|assertion| assertion.id != id);
        if state.assertions.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(())
    }

    fn results_for_run(&self, project: &str, run_id: &str) -> BackendResult<Vec<EvalResult>> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(state
            .results
            .get(&(project.to_string(), run_id.to_string()))
            .cloned()
            .unwrap_or_default())
    }

    fn evaluate<'a>(
        &'a self,
        options: &'a EvaluateOptions,
    ) -> Pin<Box<dyn Future<Output = BackendResult<EvaluateReport>> + Send + 'a>> {
        Box::pin(async move {
            let mut state = self.lock();
            if state.broken {
                return Err(broken());
            }
            let project = if options.project.is_empty() {
                "default".to_string()
            } else {
                options.project.clone()
            };
            let (candidates, mut selected) = if options.run_id.is_empty() {
                let completed: Vec<(Run, Vec<Step>)> = state
                    .runs
                    .iter()
                    .filter(|(run, _)| {
                        run.project == project
                            && run.status != Status::Running
                            && within(run, options.since, options.until)
                    })
                    .cloned()
                    .collect();
                let count = completed.len() as i64;
                (count, completed)
            } else {
                let run = state
                    .runs
                    .iter()
                    .find(|(run, _)| run.project == project && run.id == options.run_id)
                    .cloned()
                    .ok_or(BackendError::NotFound)?;
                (1, vec![run])
            };
            selected.sort_by_key(|pair| std::cmp::Reverse(pair.0.start));

            let mut run_limit = state.max_evaluate_runs.unwrap_or(500).max(1);
            if options.limit > 0 {
                run_limit = run_limit.min(options.limit);
            }
            let judge_limit = state.max_judge_calls.unwrap_or(100);
            if options.run_id.is_empty() {
                selected.truncate(run_limit.max(0) as usize);
            }

            let mut runs_evaluated = 0;
            let mut judge_calls = 0;
            let mut judge_skipped = 0;
            for (run, steps) in &selected {
                let assertions: Vec<Assertion> = state
                    .assertions
                    .iter()
                    .filter(|assertion| assertion.project == run.project && assertion.enabled)
                    .cloned()
                    .collect();
                let mut results = Vec::new();
                for assertion in &assertions {
                    if assertion.r#type == "llm_judge" {
                        let model_allowed = judge_config(&assertion.config)
                            .map(|model| state.judge_models.contains(&model))
                            .unwrap_or(false);
                        if model_allowed {
                            if judge_calls >= judge_limit {
                                judge_skipped += 1;
                                continue;
                            }
                            judge_calls += 1;
                        }
                    }
                    results.push(evaluate_assertion(assertion, run, steps));
                }
                state
                    .results
                    .insert((run.project.clone(), run.id.clone()), results);
                runs_evaluated += 1;
            }
            Ok(EvaluateReport {
                runs_evaluated,
                runs_skipped: if options.run_id.is_empty() {
                    candidates - selected.len() as i64
                } else {
                    0
                },
                run_limit,
                judge_calls,
                judge_calls_skipped: judge_skipped,
                judge_call_limit: judge_limit,
            })
        })
    }

    fn list_alerts(&self, project: &str) -> BackendResult<Vec<AlertRecord>> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(state
            .alerts
            .iter()
            .filter(|alert| project.is_empty() || alert.project == project)
            .cloned()
            .collect())
    }

    fn create_alert(&self, alert: &NewAlert) -> BackendResult<AlertRecord> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let project = if alert.project.is_empty() {
            "default".to_string()
        } else {
            alert.project.clone()
        };
        if state
            .alerts
            .iter()
            .any(|other| other.project == project && other.name == alert.name)
        {
            return Err(BackendError::Conflict);
        }
        state.next_alert_id += 1;
        let record = AlertRecord {
            id: state.next_alert_id,
            project,
            name: alert.name.clone(),
            kind: alert.kind.clone(),
            threshold: alert.threshold,
            window_secs: alert.window_secs,
            config: alert.config.clone(),
            webhook_url: alert.webhook_url.clone(),
            enabled: true,
            firing: false,
            settle_secs: alert.settle_secs,
            pending_since_ns: 0,
        };
        state.alerts.push(record.clone());
        Ok(record)
    }

    fn delete_alert(&self, id: i64) -> BackendResult<()> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let before = state.alerts.len();
        state.alerts.retain(|alert| alert.id != id);
        if state.alerts.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(())
    }

    fn create_share(&self, project: &str, run_id: &str) -> BackendResult<String> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        if state.broken {
            return Err(broken());
        }
        state.next_token += 1;
        let token = format!("{:032x}", state.next_token);
        state
            .shares
            .push((token.clone(), project.to_string(), run_id.to_string()));
        Ok(token)
    }

    fn resolve_share(&self, token: &str) -> Option<(String, String)> {
        let state = self.lock();
        state
            .shares
            .iter()
            .find(|(candidate, _, _)| candidate == token)
            .map(|(_, project, run_id)| (project.clone(), run_id.clone()))
    }

    fn shares_for_run(&self, project: &str, run_id: &str) -> BackendResult<Vec<ShareRecord>> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(state
            .shares
            .iter()
            .filter(|(_, candidate, candidate_run)| candidate == project && candidate_run == run_id)
            .map(|(token, _, _)| ShareRecord {
                token: token.clone(),
                created_at: now(),
            })
            .collect())
    }

    fn delete_share(&self, token: &str) -> BackendResult<()> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let before = state.shares.len();
        state.shares.retain(|(candidate, _, _)| candidate != token);
        if state.shares.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(())
    }

    fn list_views(&self) -> BackendResult<Vec<ViewRecord>> {
        let state = self.lock();
        if state.broken {
            return Err(broken());
        }
        Ok(state.views.clone())
    }

    fn create_view(&self, name: &str, params: &Value) -> BackendResult<ViewRecord> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        if state.views.iter().any(|view| view.name == name) {
            return Err(BackendError::Conflict);
        }
        state.next_view_id += 1;
        let view = ViewRecord {
            id: state.next_view_id,
            name: name.to_string(),
            params: params.clone(),
        };
        state.views.push(view.clone());
        Ok(view)
    }

    fn delete_view(&self, id: i64) -> BackendResult<()> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let before = state.views.len();
        state.views.retain(|view| view.id != id);
        if state.views.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(())
    }

    fn list_audit(&self, limit: i64) -> Vec<AuditRecord> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        state
            .audit
            .iter()
            .rev()
            .take(limit.max(0) as usize)
            .cloned()
            .collect()
    }

    fn list_tokens(&self) -> BackendResult<Vec<TokenRecord>> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        if state.broken {
            return Err(broken());
        }
        Ok(state
            .tokens
            .iter()
            .map(|(_, token)| token.clone())
            .collect())
    }

    fn create_token(&self, name: &str, scope: Scope) -> BackendResult<CreatedTokenRecord> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        state.next_token += 1;
        let secret = format!("{:032x}", state.next_token);
        let id = format!("tok_{}", state.next_token);
        let record = TokenRecord {
            id: id.clone(),
            name: name.to_string(),
            scope,
            created_at: now(),
            last_used_at: None,
        };
        state.tokens.push((secret.clone(), record.clone()));
        Ok(CreatedTokenRecord {
            id,
            token: secret,
            name: record.name,
            scope,
            created_at: record.created_at,
        })
    }

    fn delete_token(&self, id: &str) -> BackendResult<()> {
        let mut state = self.lock();
        if state.broken {
            return Err(broken());
        }
        let before = state.tokens.len();
        state.tokens.retain(|(_, token)| token.id != id);
        if state.tokens.len() == before {
            return Err(BackendError::NotFound);
        }
        Ok(())
    }

    fn authenticate_token(&self, token: &str) -> Option<TokenRecord> {
        let mut state = self.lock();
        state.threads.push(std::thread::current().id());
        let (_, record) = state
            .tokens
            .iter_mut()
            .find(|(secret, _)| secret == token)?;
        record.last_used_at = Some(now());
        Some(record.clone())
    }

    fn validate_assertion(&self, assertion: &Assertion) -> Result<(), String> {
        validate_assertion(assertion)
    }

    fn assertion_judge_model(&self, assertion: &Assertion) -> Result<String, String> {
        if assertion.r#type != "llm_judge" {
            return Ok(String::new());
        }
        judge_config(&assertion.config)
    }

    fn check_judge_model(&self, model: &str) -> Result<(), String> {
        let state = self.lock();
        if state
            .judge_models
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(model))
        {
            return Ok(());
        }
        let allowed = if state.judge_models.is_empty() {
            "none configured (set OTTERSCOPE_JUDGE_MODELS or judgeModels)".to_string()
        } else {
            state.judge_models.join(", ")
        };
        Err(format!(
            "judge model {model:?} is not in the judge model allow-list [{allowed}]"
        ))
    }

    fn validate_alert<'a>(
        &'a self,
        alert: &'a NewAlert,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { validate_alert(alert) })
    }

    fn redact_webhook_url(&self, raw: &str) -> String {
        redact_webhook(raw)
    }
}

// ------------------------------------------------------------------ fixtures

fn at(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().expect("timestamp")
}

fn seed_run(fake: &Fake, id: &str, start_secs: i64) {
    let base = at(start_secs);
    let run = Run {
        id: id.to_string(),
        project: "default".into(),
        service: "demo-svc".into(),
        agent_name: "demo".into(),
        status: Status::Ok,
        start: base,
        end: Some(base + chrono::Duration::seconds(9)),
        input_tokens: 100,
        output_tokens: 20,
        llm_calls: 1,
        tool_calls: 0,
        models: "claude-sonnet-5".into(),
        prompts: String::new(),
        cost_usd: None,
        cost_partial: false,
        error: String::new(),
    };
    let root = Step {
        id: format!("{id}-root"),
        project: "default".into(),
        run_id: id.to_string(),
        parent_id: String::new(),
        kind: StepKind::Agent,
        name: "invoke_agent demo".into(),
        service: "demo-svc".into(),
        agent_name: "demo".into(),
        status: Status::Ok,
        start: base,
        end: Some(base + chrono::Duration::seconds(9)),
        error: String::new(),
        llm: None,
        tool: None,
    };
    let llm = Step {
        id: format!("{id}-llm"),
        project: "default".into(),
        run_id: id.to_string(),
        parent_id: format!("{id}-root"),
        kind: StepKind::Llm,
        name: "chat m1".into(),
        service: String::new(),
        agent_name: String::new(),
        status: Status::Ok,
        start: base + chrono::Duration::seconds(1),
        end: Some(base + chrono::Duration::seconds(3)),
        error: String::new(),
        llm: Some(LlmCall {
            request_model: "claude-sonnet-5".into(),
            input_tokens: 100,
            output_tokens: 20,
            ..LlmCall::default()
        }),
        tool: None,
    };
    fake.lock().runs.push((run, vec![root, llm]));
}

fn seed_error_run(fake: &Fake, id: &str, start_secs: i64) {
    let base = at(start_secs);
    let run = Run {
        id: id.to_string(),
        project: "default".into(),
        service: "demo-svc".into(),
        agent_name: "demo".into(),
        status: Status::Error,
        start: base,
        end: Some(base + chrono::Duration::seconds(1)),
        input_tokens: 0,
        output_tokens: 0,
        llm_calls: 0,
        tool_calls: 0,
        models: String::new(),
        prompts: String::new(),
        cost_usd: None,
        cost_partial: false,
        error: "boom".into(),
    };
    let root = Step {
        id: format!("{id}-root"),
        project: "default".into(),
        run_id: id.to_string(),
        parent_id: String::new(),
        kind: StepKind::Agent,
        name: "a".into(),
        service: "demo-svc".into(),
        agent_name: "demo".into(),
        status: Status::Error,
        start: base,
        end: Some(base + chrono::Duration::seconds(1)),
        error: "boom".into(),
        llm: None,
        tool: None,
    };
    fake.lock().runs.push((run, vec![root]));
}

fn seed_detail(fake: &Fake, id: &str) {
    let base = at(1000);
    let mut state = fake.lock();
    let Some((_, steps)) = state.runs.iter_mut().find(|(run, _)| run.id == id) else {
        return;
    };
    steps.push(Step {
        id: format!("{id}-tool"),
        project: "default".into(),
        run_id: id.to_string(),
        parent_id: format!("{id}-root"),
        kind: StepKind::Tool,
        name: "execute_tool t".into(),
        service: String::new(),
        agent_name: String::new(),
        status: Status::Ok,
        start: base + chrono::Duration::seconds(4),
        end: Some(base + chrono::Duration::seconds(5)),
        error: String::new(),
        llm: None,
        tool: Some(crate::model::ToolCall {
            name: "t".into(),
            arguments: r#"{"a":1}"#.into(),
            result: r#""done""#.into(),
            ..crate::model::ToolCall::default()
        }),
    });
    for step in steps.iter_mut() {
        if step.kind == StepKind::Llm
            && let Some(llm) = step.llm.as_mut()
        {
            llm.input_messages = vec![Message {
                role: "user".into(),
                content: "hi".into(),
            }];
            llm.output_messages = vec![Message {
                role: "assistant".into(),
                content: "hello".into(),
            }];
        }
    }
}

fn app_with(fake: &Fake, options: Options) -> Router {
    build(Arc::new(fake.clone()), options)
}

fn app(fake: &Fake) -> Router {
    app_with(fake, Options::default())
}

fn base_options() -> Options {
    Options {
        base_path: BASE.into(),
        ..Options::default()
    }
}

fn auth_options() -> Options {
    Options {
        read_auth: true,
        ..Options::default()
    }
}

fn request_parts(
    method: Method,
    uri: &str,
    body: Option<&str>,
    content_type: Option<&str>,
    host: &str,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, host)
        .body(Body::from(body.unwrap_or("").to_string()))
        .expect("request");
    if let Some(body) = body {
        request.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&body.len().to_string()).expect("header"),
        );
        if let Some(content_type) = content_type {
            request.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type).expect("header"),
            );
        }
    }
    request
}

fn make_request(method: Method, uri: &str, body: Option<&str>) -> Request<Body> {
    request_parts(
        method,
        uri,
        body,
        Some("application/json"),
        "localhost:8080",
    )
}

fn make_request_ct(
    method: Method,
    uri: &str,
    body: Option<&str>,
    content_type: Option<&str>,
) -> Request<Body> {
    request_parts(method, uri, body, content_type, "localhost:8080")
}

fn make_request_host(method: Method, uri: &str, body: Option<&str>, host: &str) -> Request<Body> {
    request_parts(method, uri, body, Some("application/json"), host)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, headers, body.to_vec())
}

async fn send_head(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap) {
    let response = app.clone().oneshot(request).await.expect("response");
    (response.status(), response.headers().clone())
}

async fn send_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, _, body) = send(app, request).await;
    (status, serde_json::from_slice(&body).expect("json body"))
}

async fn send_status(app: &Router, request: Request<Body>) -> StatusCode {
    send(app, request).await.0
}

fn body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

async fn token_send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut request = make_request(method, uri, body);
    if let Some(token) = token {
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
        );
    }
    let (status, _, body) = send(app, request).await;
    (status, body)
}

fn runs_of(body: &Value) -> Vec<Value> {
    body["runs"].as_array().cloned().unwrap_or_default()
}

// ------------------------------------------------------------------ guard

#[tokio::test]
async fn cross_origin_write_refused() {
    let fake = Fake::new();
    let app = app(&fake);
    for origin in ["https://evil.example", "http://attacker.test:8080", "null"] {
        let mut request = make_request(
            Method::POST,
            "/api/views",
            Some(r#"{"name":"evil","params":{}}"#),
        );
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_str(origin).expect("origin"),
        );
        let (status, _, body) = send(&app, request).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "origin {origin}: {}",
            body_text(&body)
        );
    }

    let (status, body) = send_json(&app, make_request(Method::GET, "/api/views", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["views"].as_array().map(Vec::len), Some(0));

    let mut request = make_request(Method::GET, "/api/runs", None);
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.example"),
    );
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rebound_host_refused() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, _, body) = send(
        &app,
        make_request_host(Method::GET, "/api/runs", None, "evil.example:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{}", body_text(&body));
}

#[tokio::test]
async fn non_json_write_refused() {
    let fake = Fake::new();
    let app = app(&fake);
    for content_type in [
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data; boundary=x"),
        None,
    ] {
        let (status, _, body) = send(
            &app,
            make_request_ct(
                Method::POST,
                "/api/views",
                Some(r#"{"name":"plain","params":{}}"#),
                content_type,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content type {content_type:?}: {}",
            body_text(&body)
        );
    }
    let (_, body) = send_json(&app, make_request(Method::GET, "/api/views", None)).await;
    assert_eq!(body["views"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn same_origin_json_write_accepted() {
    let fake = Fake::new();
    let app = app(&fake);
    let mut request = make_request_ct(
        Method::POST,
        "/api/views",
        Some(r#"{"name":"mine","params":{"range":"24h"}}"#),
        Some("application/json; charset=utf-8"),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://localhost:8080"),
    );
    let (status, body) = send_json(&app, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_i64().expect("id");

    let mut request = make_request_ct(Method::DELETE, &format!("/api/views/{id}"), None, None);
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://localhost:8080"),
    );
    let status = send_status(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = send_json(&app, make_request(Method::GET, "/api/views", None)).await;
    assert_eq!(body["views"].as_array().map(Vec::len), Some(0));
}

#[test]
fn options_from_config_copy_the_resolved_values() {
    let config = crate::config::Config {
        read_auth: true,
        base_path: "/flagbase".into(),
        allowed_origins: "https://obs.corp.example".into(),
        judge_models: vec!["gpt-4o-mini".into()],
        judge_url: "https://judge.example/v1".into(),
        judge_key: "sk-test".into(),
        ..crate::config::Config::default()
    };
    let options = Options::from_config(&config);
    assert!(options.read_auth);
    assert_eq!(options.base_path, "/flagbase");
    assert_eq!(options.allowed_origins, "https://obs.corp.example");
    assert_eq!(options.judge_models, vec!["gpt-4o-mini"]);
    assert_eq!(options.judge_url, "https://judge.example/v1");
    assert_eq!(options.judge_key, "sk-test");
}

#[tokio::test]
async fn security_headers_set() {
    let fake = Fake::new();
    seed_run(&fake, "h1", 1000);
    let app = app(&fake);
    let cases: [(Method, &str, Option<&str>, Option<&str>); 4] = [
        (Method::GET, "/", None, None),
        (Method::GET, "/api/runs", None, None),
        (Method::GET, "/api/nope", None, None),
        (
            Method::POST,
            "/api/views",
            Some("not json"),
            Some("text/plain"),
        ),
    ];
    for (method, uri, body, content_type) in cases {
        let (_, headers, _) = send(
            &app,
            make_request_ct(method.clone(), uri, body, content_type),
        )
        .await;
        assert_eq!(
            headers
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some(super::guard::CONTENT_SECURITY_POLICY),
            "{method} {uri}"
        );
        assert_eq!(
            headers
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff"),
            "{method} {uri}"
        );
        assert_eq!(
            headers
                .get(header::REFERRER_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some("no-referrer"),
            "{method} {uri}"
        );
    }
    let (status, headers, _) = send(&app, make_request(Method::GET, "/", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
}

#[tokio::test]
async fn unknown_api_path_is_json_404() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, headers, body) = send(&app, make_request(Method::GET, "/api/nope", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body: Value = serde_json::from_slice(&body).expect("JSON envelope");
    assert_eq!(body["error"], "not found");
}

#[tokio::test]
async fn security_headers_cover_auth_failures_and_prefix_misses() {
    let fake = Fake::new();
    let options = Options {
        read_auth: true,
        base_path: BASE.into(),
        ..Options::default()
    };
    let app = app_with(&fake, options);
    for (uri, expected) in [
        (format!("{BASE}/api/runs"), StatusCode::UNAUTHORIZED),
        ("/api/runs".to_string(), StatusCode::NOT_FOUND),
    ] {
        let (status, headers, _) = send(&app, make_request(Method::GET, &uri, None)).await;
        assert_eq!(status, expected, "{uri}");
        assert_eq!(
            headers
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some(super::guard::CONTENT_SECURITY_POLICY),
            "{uri}"
        );
        assert_eq!(
            headers
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff"),
            "{uri}"
        );
        assert_eq!(
            headers
                .get(header::REFERRER_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some("no-referrer"),
            "{uri}"
        );
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store"),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn json_responses_forbid_caching() {
    let fake = Fake::new();
    seed_run(&fake, "c1", 1000);
    let app = app(&fake);

    let (status, headers, _) = send(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let (status, headers, body) =
        send(&app, make_request(Method::GET, "/api/runs/missing", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert!(body_text(&body).contains("run not found"));
}

#[tokio::test]
async fn metrics_requires_token() {
    let fake = Fake::new();
    seed_run(&fake, "m1", 1000);
    let token = fake.create_token("prometheus", Scope::Read).expect("token");
    let app = app_with(&fake, auth_options());

    let (status, _, _) = send(&app, make_request(Method::GET, "/metrics", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = token_send(&app, Method::GET, "/metrics", Some(&token.token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body_text(&body).contains("otterscope_runs_total"));
    let (status, _, _) = send(&app, make_request(Method::GET, "/healthz", None)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn configured_origin_is_accepted() {
    let fake = Fake::new();
    let options = Options {
        allowed_origins: "https://obs.corp.example".into(),
        ..Options::default()
    };
    let app = app_with(&fake, options);

    let mut request = make_request_host(Method::GET, "/api/runs", None, "obs.corp.example");
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://obs.corp.example"),
    );
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));

    let mut request = make_request_host(Method::GET, "/api/runs", None, "obs.corp.example");
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://elsewhere.example"),
    );
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn guard_under_base_path() {
    let fake = Fake::new();
    let app = app_with(&fake, base_options());
    let mut request = make_request(
        Method::POST,
        &format!("{BASE}/api/views"),
        Some(r#"{"name":"x","params":{}}"#),
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.example"),
    );
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{}", body_text(&body));
}

#[tokio::test]
async fn lan_address_accepted() {
    let fake = Fake::new();
    let app = app(&fake);

    let (status, _, body) = send(
        &app,
        make_request_host(Method::GET, "/api/runs", None, "192.168.1.10:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&body));

    let mut request = make_request_host(Method::GET, "/api/runs", None, "192.168.1.10:8080");
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://192.168.1.10:8080"),
    );
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK);

    let mut request = make_request_host(
        Method::POST,
        "/api/views",
        Some(r#"{"name":"x","params":{}}"#),
        "192.168.1.10:8080",
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://203.0.113.5"),
    );
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------ base path

#[tokio::test]
async fn base_path_api_route_under_prefix() {
    let fake = Fake::new();
    seed_run(&fake, "r-old", 1000);
    seed_run(&fake, "r-new", 2000);

    let root = app(&fake);
    let prefixed = app_with(&fake, base_options());

    let (status, _, prefixed_body) = send(
        &prefixed,
        make_request(Method::GET, &format!("{BASE}/api/runs"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, root_body) = send(&root, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prefixed_body, root_body);

    let body: Value = serde_json::from_slice(&prefixed_body).expect("json");
    let runs = runs_of(&body);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["id"], "r-new");
    assert_eq!(runs[1]["id"], "r-old");
}

#[tokio::test]
async fn base_path_root_is_not_found() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app_with(&fake, base_options());

    for (method, path) in [
        (Method::GET, "/api/runs".to_string()),
        (Method::GET, "/healthz".to_string()),
        (Method::GET, "/metrics".to_string()),
        (Method::POST, "/mcp".to_string()),
        (Method::GET, format!("{BASE}api")),
        (Method::GET, "/".to_string()),
        (Method::GET, "/somewhere/unknown".to_string()),
    ] {
        let (status, _, _) = send(&app, make_request(method.clone(), &path, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let (status, _, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/api/runs"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/healthz"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn base_path_serves_spa_index() {
    let fake = Fake::new();
    let app = app_with(&fake, base_options());
    let (status, headers, body) =
        send(&app, make_request(Method::GET, &format!("{BASE}/"), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
    let text = body_text(&body);
    assert!(
        text.contains("<html") || text.contains("<!doctype"),
        "index body: {text}"
    );
}

#[test]
fn rewrite_asset_refs_is_idempotent() {
    let html =
        r#"<script src="/assets/index-abc.js"></script><link href="/assets/x.css"><a href="/s/t">"#;
    let prefixed = super::ui::rewrite_asset_refs(html, BASE);
    assert!(prefixed.contains(r#"src="/otterscope/assets/index-abc.js""#));
    assert!(prefixed.contains(r#"href="/otterscope/assets/x.css""#));
    assert!(prefixed.contains(r#"href="/otterscope/s/t""#));
    assert!(!prefixed.contains(r#""/assets/"#));
    assert_eq!(super::ui::rewrite_asset_refs(&prefixed, BASE), prefixed);
    assert_eq!(super::ui::rewrite_asset_refs(html, ""), html);
}

#[tokio::test]
async fn base_path_spa_fallback_stays_under_prefix() {
    let fake = Fake::new();
    let app = app_with(&fake, base_options());
    let (status, headers, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/somewhere/unknown"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
    let (status, _, _) = send(&app, make_request(Method::GET, "/somewhere/unknown", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn base_path_share_link_is_prefixed() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app_with(&fake, base_options());

    let (status, body) = send_json(
        &app,
        make_request(Method::POST, &format!("{BASE}/api/runs/r1/share"), None),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["token"].as_str().expect("token");
    let url = body["url"].as_str().expect("url");
    assert_eq!(url, format!("{BASE}/s/{token}"));

    let (status, _, _) = send(&app, make_request(Method::GET, url, None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send_json(
        &app,
        make_request(Method::GET, &format!("{BASE}/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["id"], "r1");
}

#[tokio::test]
async fn base_path_auth_applies_to_prefix() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let token = fake.create_token("script", Scope::Read).expect("token");
    let options = Options {
        read_auth: true,
        base_path: BASE.into(),
        ..Options::default()
    };
    let app = app_with(&fake, options);

    let (status, _, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/api/runs"), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = token_send(
        &app,
        Method::GET,
        &format!("{BASE}/api/runs"),
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = token_send(
        &app,
        Method::GET,
        &format!("{BASE}/api/runs"),
        Some(&token.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/healthz"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn base_path_normalized_in_options() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    for spelling in ["/otterscope/", "otterscope", "  /otterscope  "] {
        let options = Options {
            base_path: spelling.into(),
            ..Options::default()
        };
        let app = app_with(&fake, options);
        let (status, _, _) = send(
            &app,
            make_request(Method::GET, &format!("{BASE}/api/runs"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "base {spelling:?}");
        let (status, _, _) = send(
            &app,
            make_request(Method::GET, &format!("{BASE}/healthz"), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "base {spelling:?}");
    }
}

#[tokio::test]
async fn base_path_outside_mount_is_404_not_unauthorized() {
    let fake = Fake::new();
    let token = fake.create_token("script", Scope::Read).expect("token");
    let options = Options {
        read_auth: true,
        base_path: BASE.into(),
        ..Options::default()
    };
    let app = app_with(&fake, options);
    for bearer in [None, Some(token.token.as_str())] {
        let (status, _) = token_send(&app, Method::GET, "/api/runs", bearer, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn base_path_redirect_stays_inside_prefix() {
    let fake = Fake::new();
    let app = app_with(&fake, base_options());
    let expected = format!("{BASE}/b");

    let (status, headers, _) = send(
        &app,
        make_request(Method::GET, &format!("{BASE}/a/../b"), None),
    )
    .await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(expected.as_str())
    );

    let (status, headers, _) =
        send(&app, make_request(Method::GET, &format!("{BASE}//b"), None)).await;
    assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(expected.as_str())
    );
}

// ------------------------------------------------- one app: UI/API + OTLP

const INGEST_KEY: &str = "ingest-key";

#[derive(Default)]
struct RecordingSink {
    seen: Mutex<Vec<(String, usize)>>,
}

impl crate::ingest::Sink for RecordingSink {
    fn consume(
        &self,
        project: &str,
        _raw: &[u8],
        steps: &[Step],
    ) -> anyhow::Result<crate::ingest::IngestResult> {
        self.seen
            .lock()
            .expect("sink")
            .push((project.to_string(), steps.len()));
        Ok(crate::ingest::IngestResult::default())
    }
}

/// The receiver `serve` mounts: root-relative, because the app's edge strips
/// the base path once before routing.
fn ingest_router(sink: Arc<RecordingSink>, require_key: bool) -> Router {
    let resolve: crate::ingest::KeyResolver = Arc::new(|key: &str| match key {
        "" => Some("default".to_string()),
        INGEST_KEY => Some("prod".to_string()),
        _ => None,
    });
    crate::ingest::router(
        crate::ingest::Options {
            base_path: String::new(),
            require_key,
            ..Default::default()
        },
        sink,
        resolve,
    )
}

fn otlp_fixture() -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/pydantic_ai_chat.json");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()))
}

fn otlp_request(uri: &str, body: &str) -> Request<Body> {
    request_parts(
        Method::POST,
        uri,
        Some(body),
        Some("application/json"),
        "localhost:8080",
    )
}

#[tokio::test]
async fn ingest_and_api_are_served_by_one_app() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let sink = Arc::new(RecordingSink::default());
    let app = build_with_ingest(
        Arc::new(fake.clone()),
        Options::default(),
        Hub::new(),
        ingest_router(sink.clone(), false),
    );

    let body = otlp_fixture();
    let (status, headers, bytes) = send(&app, otlp_request("/v1/traces", &body)).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&bytes));
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        sink.seen.lock().expect("sink").as_slice(),
        &[("default".to_string(), 3)],
        "OTLP reached the ingest sink"
    );

    // The very same Router value serves the JSON API; no second app/listener.
    let (status, runs) = send_json(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runs_of(&runs).len(), 1);
}

#[tokio::test]
async fn base_path_prefixes_ingest_and_api_on_one_app() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let sink = Arc::new(RecordingSink::default());
    let app = build_with_ingest(
        Arc::new(fake.clone()),
        base_options(),
        Hub::new(),
        ingest_router(sink.clone(), false),
    );

    let body = otlp_fixture();
    let (status, _, bytes) = send(&app, otlp_request(&format!("{BASE}/v1/traces"), &body)).await;
    assert_eq!(status, StatusCode::OK, "{}", body_text(&bytes));
    assert_eq!(sink.seen.lock().expect("sink")[0].1, 3);

    let (status, runs) = send_json(
        &app,
        make_request(Method::GET, &format!("{BASE}/api/runs"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runs_of(&runs).len(), 1);

    // Outside the prefix neither route exists, and OTLP never reaches the sink.
    let (status, _, _) = send(&app, otlp_request("/v1/traces", &body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(sink.seen.lock().expect("sink").len(), 1);
}

#[tokio::test]
async fn ingest_keeps_its_own_auth_when_read_auth_is_on() {
    let fake = Fake::new();
    let sink = Arc::new(RecordingSink::default());
    let app = build_with_ingest(
        Arc::new(fake.clone()),
        auth_options(),
        Hub::new(),
        ingest_router(sink.clone(), true),
    );

    let body = otlp_fixture();
    // OTLP is not gated by the UI/API read token: it asks for its own ingest
    // key inside the receiver.
    let (status, _, bytes) = send(&app, otlp_request("/v1/traces", &body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body_text(&bytes).contains("ingest key required"));

    // The UI/API still requires a read token.
    let (status, _, _) = send(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A project ingest key authenticates OTLP even though no read token is sent.
    let mut request = otlp_request("/v1/traces", &body);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer ingest-key"),
    );
    let (status, _, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sink.seen.lock().expect("sink")[0].0, "prod");
}

// ------------------------------------------------------------------ API

#[tokio::test]
async fn list_runs_api() {
    let fake = Fake::new();
    seed_run(&fake, "r-old", 1000);
    seed_run(&fake, "r-new", 2000);
    let app = app(&fake);

    let (status, body) = send_json(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    let runs = runs_of(&body);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["id"], "r-new");
    assert_eq!(runs[1]["id"], "r-old");
    let run = &runs[0];
    assert_eq!(run["status"], "ok");
    assert_eq!(run["service"], "demo-svc");
    assert_eq!(run["agentName"], "demo");
    assert_eq!(run["durationMs"], 9000);
    assert_eq!(run["inputTokens"], 100);
    assert_eq!(run["outputTokens"], 20);
    assert_eq!(run["models"], "claude-sonnet-5");
    assert_eq!(run["llmCalls"], 1);
    assert_eq!(run["start"], "1970-01-01T00:33:20.000Z");
    assert!(run.get("costUsd").is_none());
}

#[tokio::test]
async fn list_runs_paging_and_clamping() {
    let fake = Fake::new();
    for (index, id) in ["ra", "rb", "rc"].iter().enumerate() {
        seed_run(&fake, id, 1000 + index as i64 * 100);
    }
    let app = app(&fake);

    let (_, body) = send_json(&app, make_request(Method::GET, "/api/runs?limit=2", None)).await;
    assert_eq!(runs_of(&body).len(), 2);
    let (_, body) = send_json(
        &app,
        make_request(Method::GET, "/api/runs?limit=2&offset=2", None),
    )
    .await;
    assert_eq!(runs_of(&body).len(), 1);
    let (_, body) = send_json(
        &app,
        make_request(Method::GET, "/api/runs?limit=banana&offset=-5", None),
    )
    .await;
    assert_eq!(runs_of(&body).len(), 3);
}

#[tokio::test]
async fn list_runs_empty() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, body) = send_json(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runs_of(&body).len(), 0);
}

#[tokio::test]
async fn get_run_api() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    seed_detail(&fake, "r1");
    let app = app(&fake);

    let (status, body) = send_json(&app, make_request(Method::GET, "/api/runs/r1", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["id"], "r1");
    let steps = body["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3);
    let llm = steps
        .iter()
        .find(|step| step["kind"] == "llm")
        .expect("llm step");
    assert_eq!(llm["llm"]["inputMessages"][0]["content"], "hi");
    assert_eq!(llm["offsetMs"], 1000);
    assert_eq!(llm["durationMs"], 2000);
    let tool = steps
        .iter()
        .find(|step| step["kind"] == "tool")
        .expect("tool step");
    assert_eq!(tool["tool"]["arguments"], r#"{"a":1}"#);
    assert_eq!(tool["tool"]["result"], r#""done""#);
}

#[tokio::test]
async fn get_run_not_found() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, body) = send_json(&app, make_request(Method::GET, "/api/runs/nope", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "run not found");
    let (status, _) = send_json(
        &app,
        make_request(Method::GET, "/api/runs/nope/shares", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn assertions_api() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app(&fake);

    let (status, _) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions",
            Some(r#"{"name":"fast","type":"max_latency_ms","config":"20000"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions",
            Some(r#"{"name":"bad","type":"regex","config":"("}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions/evaluate?project=default&run=r1",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["runsEvaluated"], 1);

    let (_, body) = send_json(&app, make_request(Method::GET, "/api/runs/r1", None)).await;
    let results = body["assertionResults"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], "fast");
    assert_eq!(results[0]["pass"], true);
}

#[tokio::test]
async fn share_lifecycle() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app(&fake);

    let (status, mint) =
        send_json(&app, make_request(Method::POST, "/api/runs/r1/share", None)).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = mint["token"].as_str().expect("token");
    assert_eq!(token.len(), 32);
    assert_eq!(mint["url"], format!("/s/{token}"));

    let (status, shared) = send_json(
        &app,
        make_request(Method::GET, &format!("/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["run"]["id"], "r1");
    assert!(shared.get("assertionResults").is_none());

    let (_, list) = send_json(&app, make_request(Method::GET, "/api/runs/r1/shares", None)).await;
    let shares = list["shares"].as_array().expect("shares");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["token"], token);

    let status = send_status(
        &app,
        make_request(Method::DELETE, &format!("/api/shares/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send_json(
        &app,
        make_request(Method::GET, &format!("/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shared_unknown_token() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, _) = send_json(
        &app,
        make_request(Method::GET, "/api/shared/deadbeef", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn saved_views_api() {
    let fake = Fake::new();
    let app = app(&fake);
    let body = r#"{"name":"prod errors","params":{"status":"error","range":"24h","q":"timeout"}}"#;
    let (status, created) =
        send_json(&app, make_request(Method::POST, "/api/views", Some(body))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "prod errors");
    assert_eq!(created["params"]["status"], "error");
    assert_eq!(created["params"]["q"], "timeout");

    let (status, _) = send_json(&app, make_request(Method::POST, "/api/views", Some(body))).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (_, list) = send_json(&app, make_request(Method::GET, "/api/views", None)).await;
    let views = list["views"].as_array().expect("views");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0]["params"]["range"], "24h");

    let id = created["id"].as_i64().expect("id");
    let status = send_status(
        &app,
        make_request(Method::DELETE, &format!("/api/views/{id}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send_json(&app, make_request(Method::GET, "/api/views", None)).await;
    assert_eq!(list["views"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn read_auth() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let token = fake.create_token("script", Scope::Read).expect("token");
    let app = app_with(&fake, auth_options());

    let (status, _) = token_send(&app, Method::GET, "/api/runs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = token_send(&app, Method::GET, "/api/runs", Some("nope"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = token_send(&app, Method::GET, "/api/runs", Some(&token.token), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app, make_request(Method::GET, "/healthz", None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = token_send(&app, Method::GET, "/api/shared/whatever", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send_head(
        &app,
        make_request(
            Method::GET,
            &format!("/api/stream?token={}", token.token),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = token_send(&app, Method::GET, "/api/stream?token=bad", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Query-string auth is only for EventSource; other reads need the header.
    let (status, _) = token_send(
        &app,
        Method::GET,
        &format!("/api/runs?token={}", token.token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn metrics_endpoint() {
    let fake = Fake::new();
    seed_run(&fake, "ok1", 1000);
    seed_error_run(&fake, "err1", 2000);
    let app = app(&fake);

    let (status, _, body) = send(&app, make_request(Method::GET, "/metrics", None)).await;
    assert_eq!(status, StatusCode::OK);
    let text = body_text(&body);
    for expected in [
        "otterscope_build_info{version=",
        "otterscope_runs_total 2",
        r#"otterscope_runs{status="ok"} 1"#,
        r#"otterscope_runs{status="error"} 1"#,
        "otterscope_db_size_bytes ",
        "# TYPE otterscope_steps_total gauge",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
}

#[tokio::test]
async fn runs_csv() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    seed_run(&fake, "r2", 2000);
    let app = app(&fake);

    let (status, headers, body) =
        send(&app, make_request(Method::GET, "/api/runs.csv", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/csv"))
    );
    assert!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("otterscope-runs.csv"))
    );
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let text = body_text(&body);
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].starts_with("id,project,service"));
    assert!(rows[1].starts_with("r2,"));

    let (_, _, body) = send(
        &app,
        make_request(Method::GET, "/api/runs.csv?status=error", None),
    )
    .await;
    assert_eq!(body_text(&body).lines().count(), 1);
}

#[tokio::test]
async fn malformed_time_bound_is_rejected() {
    let fake = Fake::new();
    seed_run(&fake, "r-1", 1000);
    seed_run(&fake, "r-2", 2000);
    let app = app(&fake);
    for path in ["/api/runs", "/api/stats", "/api/runs.csv"] {
        for query in [
            "?since=nonsense",
            "?until=2026-08-21",
            "?since=2026-13-01T00:00:00Z",
        ] {
            let (status, _, body) = send(
                &app,
                make_request(Method::GET, &format!("{path}{query}"), None),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{path}{query}: {}",
                body_text(&body)
            );
        }
    }
    let (_, body) = send_json(
        &app,
        make_request(Method::GET, "/api/runs?since=1970-01-01T00:16:40Z", None),
    )
    .await;
    assert_eq!(runs_of(&body).len(), 2);
    let (_, body) = send_json(&app, make_request(Method::GET, "/api/runs?since=", None)).await;
    assert_eq!(runs_of(&body).len(), 2);
}

#[tokio::test]
async fn unknown_status_is_a_client_error() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, body) = send_json(
        &app,
        make_request(Method::GET, "/api/runs?status=weird", None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("unknown status")
    );
}

#[tokio::test]
async fn delete_unknown_id_is_404() {
    let fake = Fake::new();
    let app = app(&fake);
    for target in [
        "/api/views/99999",
        "/api/assertions/99999",
        "/api/alerts/99999",
        "/api/tokens/no-such-token",
        "/api/shares/no-such-token",
    ] {
        let status = send_status(&app, make_request(Method::DELETE, target, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "DELETE {target}");
    }
}

#[tokio::test]
async fn delete_existing_is_204() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, view) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/views",
            Some(r#"{"name":"mine","params":{}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let target = format!("/api/views/{}", view["id"].as_i64().expect("id"));
    let status = send_status(&app, make_request(Method::DELETE, &target, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = send_status(&app, make_request(Method::DELETE, &target, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn duplicate_name_is_conflict() {
    let fake = Fake::new();
    let app = app(&fake);
    let body = r#"{"name":"dupe","params":{}}"#;
    let (status, _) = send_json(&app, make_request(Method::POST, "/api/views", Some(body))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, error) =
        send_json(&app, make_request(Method::POST, "/api/views", Some(body))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let text = error.to_string();
    assert!(text.contains("dupe"), "{text}");
    assert!(!text.contains("UNIQUE"), "{text}");
}

#[tokio::test]
async fn oversized_body_is_rejected_with_the_json_envelope() {
    let fake = Fake::new();
    let app = app(&fake);
    let body = format!(
        r#"{{"name":"{}","params":{{}}}}"#,
        "x".repeat(super::MAX_BODY_BYTES)
    );
    let (status, headers, body) =
        send(&app, make_request(Method::POST, "/api/views", Some(&body))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some(super::guard::CONTENT_SECURITY_POLICY)
    );
    let body: Value = serde_json::from_slice(&body).expect("JSON envelope");
    assert_eq!(body["error"], "request body too large");
}

// ------------------------------------------------------------- blocking pool

fn worker_thread() -> std::thread::ThreadId {
    std::thread::current().id()
}

fn assert_off_the_async_worker(fake: &Fake) {
    let threads = fake.lock().threads.clone();
    assert!(!threads.is_empty(), "no backend call was recorded");
    let worker = worker_thread();
    for thread in threads {
        assert_ne!(
            thread, worker,
            "a backend call ran on the async worker thread"
        );
    }
}

#[tokio::test]
async fn handlers_run_store_work_off_the_async_worker() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app(&fake);
    fake.lock().threads.clear();

    let (status, _) = send_json(&app, make_request(Method::GET, "/api/runs", None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_json(&app, make_request(Method::GET, "/api/audit", None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_json(&app, make_request(Method::GET, "/api/tokens", None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_json(&app, make_request(Method::POST, "/api/runs/r1/share", None)).await;
    assert_eq!(status, StatusCode::CREATED);

    assert_off_the_async_worker(&fake);
}

#[tokio::test]
async fn auth_lookup_runs_off_the_async_worker() {
    let fake = Fake::new();
    let token = fake.create_token("operator", Scope::Admin).expect("token");
    fake.lock().threads.clear();
    let app = app_with(&fake, auth_options());

    let (status, _) = token_send(&app, Method::GET, "/api/runs", Some(&token.token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_off_the_async_worker(&fake);
}

#[tokio::test]
async fn store_failure_is_internal_error() {
    let fake = Fake::new();
    fake.lock().broken = true;
    let app = app(&fake);
    let (status, body) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/views",
            Some(r#"{"name":"x","params":{}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!body.to_string().contains("already exists"));
}

#[tokio::test]
async fn csv_truncation_is_reported() {
    let fake = Fake::new();
    for (index, id) in ["c1", "c2", "c3"].iter().enumerate() {
        seed_run(&fake, id, 1000 + index as i64);
    }

    let options = Options {
        csv_export_limit: 2,
        ..Options::default()
    };
    let app = app_with(&fake, options);
    let (status, headers, body) =
        send(&app, make_request(Method::GET, "/api/runs.csv", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("x-otterscope-truncated")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    assert_eq!(
        headers
            .get("x-otterscope-row-limit")
            .and_then(|value| value.to_str().ok()),
        Some("2")
    );
    assert_eq!(body_text(&body).lines().count(), 3);

    let app = app_with(
        &fake,
        Options {
            csv_export_limit: 10,
            ..Options::default()
        },
    );
    let (_, headers, body) = send(&app, make_request(Method::GET, "/api/runs.csv", None)).await;
    assert!(headers.get("x-otterscope-truncated").is_none());
    assert_eq!(body_text(&body).lines().count(), 4);
}

#[tokio::test]
async fn csv_cost_is_fixed_decimal_without_float_noise() {
    let fake = Fake::new();
    seed_run(&fake, "r-cost", 1000);
    fake.lock().runs[0].0.cost_usd = Some(0.005_031_199_999_999_999_6);
    let app = app(&fake);

    let (status, _, body) = send(&app, make_request(Method::GET, "/api/runs.csv", None)).await;
    assert_eq!(status, StatusCode::OK);
    let text = body_text(&body);
    let mut rows = text.lines();
    let header: Vec<&str> = rows.next().expect("header").split(',').collect();
    let row: Vec<&str> = rows.next().expect("row").split(',').collect();
    let cost = header
        .iter()
        .position(|name| *name == "cost_usd")
        .expect("cost column");
    assert_eq!(row[cost], "0.00503");
    assert!(!text.contains("0.0050311999999999996"), "{text}");
}

// ------------------------------------------------------------------ auth/admin

fn auth_app() -> (Fake, Router, CreatedTokenRecord, CreatedTokenRecord) {
    let fake = Fake::new();
    let read = fake
        .create_token("reader", Scope::Read)
        .expect("read token");
    let admin = fake
        .create_token("operator", Scope::Admin)
        .expect("admin token");
    let app = app_with(&fake, auth_options());
    (fake, app, read, admin)
}

#[tokio::test]
async fn read_token_cannot_mutate() {
    let (_fake, app, read, admin) = auth_app();
    let mutations = [
        (Method::POST, "/api/tokens", Some(r#"{"name":"minted"}"#)),
        (Method::DELETE, "/api/tokens/tok_nope", None),
        (
            Method::POST,
            "/api/assertions",
            Some(r#"{"project":"default","name":"a","type":"contains","config":"ok"}"#),
        ),
        (Method::POST, "/api/assertions/evaluate", Some("{}")),
        (Method::DELETE, "/api/assertions/1", None),
        (
            Method::POST,
            "/api/alerts",
            Some(r#"{"project":"default","name":"a","type":"error_rate","threshold":0.5}"#),
        ),
        (Method::DELETE, "/api/alerts/1", None),
        (
            Method::POST,
            "/api/views",
            Some(r#"{"name":"v","params":{}}"#),
        ),
        (Method::DELETE, "/api/views/1", None),
        (Method::POST, "/api/runs/r1/share", None),
        (Method::DELETE, "/api/shares/tok_nope", None),
    ];
    for (method, path, body) in mutations {
        let (status, payload) =
            token_send(&app, method.clone(), path, Some(&read.token), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} with read token: {}",
            body_text(&payload)
        );
        let (status, payload) =
            token_send(&app, method.clone(), path, Some(&admin.token), body).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} with admin token: {}",
            body_text(&payload)
        );
    }
    let (status, _) = token_send(&app, Method::GET, "/api/tokens", Some(&read.token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in [
        "/api/runs",
        "/api/stats",
        "/api/views",
        "/api/assertions",
        "/api/alerts",
    ] {
        let (status, _) = token_send(&app, Method::GET, path, Some(&read.token), None).await;
        assert_eq!(status, StatusCode::OK, "GET {path}");
    }
}

#[tokio::test]
async fn share_mutations_require_admin() {
    let (fake, app, read, admin) = auth_app();
    seed_run(&fake, "r1", 1000);

    let (status, payload) = token_send(
        &app,
        Method::POST,
        "/api/runs/r1/share",
        Some(&read.token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "read token minted a share: {}",
        body_text(&payload)
    );

    let (status, payload) = token_send(
        &app,
        Method::POST,
        "/api/runs/r1/share",
        Some(&admin.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", body_text(&payload));
    let minted: Value = serde_json::from_slice(&payload).expect("json");
    let token = minted["token"].as_str().expect("token").to_string();

    // Publishing is a mutation, but reading the published run stays public.
    let (status, shared) = send_json(
        &app,
        make_request(Method::GET, &format!("/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["run"]["id"], "r1");

    let (status, payload) = token_send(
        &app,
        Method::DELETE,
        &format!("/api/shares/{token}"),
        Some(&read.token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "read token revoked a share: {}",
        body_text(&payload)
    );
    let (status, _) = send_json(
        &app,
        make_request(Method::GET, &format!("/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the refused revoke still took effect"
    );

    let (status, _) = token_send(
        &app,
        Method::DELETE,
        &format!("/api/shares/{token}"),
        Some(&admin.token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send_json(
        &app,
        make_request(Method::GET, &format!("/api/shared/{token}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn token_listing_leaks_no_secret() {
    let (fake, app, _read, admin) = auth_app();
    let created = fake.create_token("ci", Scope::Read).expect("token");
    let (status, body) =
        token_send(&app, Method::GET, "/api/tokens", Some(&admin.token), None).await;
    assert_eq!(status, StatusCode::OK);
    let text = body_text(&body);
    assert!(
        !text.contains(&created.token),
        "listing leaked a credential: {text}"
    );
    assert!(!text.contains(&admin.token));
    let list: Value = serde_json::from_slice(&body).expect("json");
    let tokens = list["tokens"].as_array().expect("tokens");
    assert_eq!(tokens.len(), 3);
    let ci = tokens
        .iter()
        .find(|token| token["name"] == "ci")
        .expect("ci token");
    assert!(ci.get("token").is_none());
    assert_eq!(ci["id"], created.id);
    assert_eq!(ci["scope"], "read");
    assert!(ci.get("createdAt").is_some());
}

#[tokio::test]
async fn create_token_response_carries_the_secret() {
    let (_fake, app, _read, admin) = auth_app();
    let mut request = make_request(Method::POST, "/api/tokens", Some(r#"{"name":"ci"}"#));
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", admin.token)).expect("header"),
    );
    let (status, created) = send_json(&app, request).await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().expect("secret");
    assert!(!secret.is_empty());
    assert_ne!(created["id"], secret);
    assert_eq!(created["scope"], "read");

    let (status, _) = token_send(&app, Method::GET, "/api/runs", Some(secret), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = token_send(
        &app,
        Method::POST,
        "/api/views",
        Some(secret),
        Some(r#"{"name":"v","params":{}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, list) = token_send(&app, Method::GET, "/api/tokens", Some(&admin.token), None).await;
    assert!(!body_text(&list).contains(secret));
}

#[tokio::test]
async fn delete_token_by_id_revokes() {
    let (fake, app, _read, admin) = auth_app();
    let victim = fake.create_token("old-ci", Scope::Read).expect("token");
    let path = format!("/api/tokens/{}", victim.id);
    let (status, _) = token_send(&app, Method::DELETE, &path, Some(&admin.token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = token_send(&app, Method::GET, "/api/runs", Some(&victim.token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = token_send(&app, Method::DELETE, &path, Some(&admin.token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn token_last_used_over_api() {
    let (fake, app, _read, admin) = auth_app();
    let dashboard = fake.create_token("dashboard", Scope::Read).expect("token");

    let before = {
        let (_, body) =
            token_send(&app, Method::GET, "/api/tokens", Some(&admin.token), None).await;
        serde_json::from_slice::<Value>(&body).expect("json")
    };
    let entry = before["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|token| token["id"] == dashboard.id)
        .expect("dashboard");
    assert!(entry.get("lastUsedAt").is_none());

    let (status, _) =
        token_send(&app, Method::GET, "/api/runs", Some(&dashboard.token), None).await;
    assert_eq!(status, StatusCode::OK);

    let after = {
        let (_, body) =
            token_send(&app, Method::GET, "/api/tokens", Some(&admin.token), None).await;
        serde_json::from_slice::<Value>(&body).expect("json")
    };
    let entry = after["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|token| token["id"] == dashboard.id)
        .expect("dashboard");
    assert!(entry.get("lastUsedAt").is_some());
}

// ------------------------------------------------------------------ evaluate

#[tokio::test]
async fn evaluate_refuses_unbounded_call() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app(&fake);
    let (status, body) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions/evaluate?project=default",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().expect("error");
    for expected in ["run", "since", "limit"] {
        assert!(error.contains(expected), "{error}");
    }
}

#[tokio::test]
async fn evaluate_bounded_call_processed() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    seed_run(&fake, "r2", 2000);
    let app = app(&fake);
    let (status, _) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions",
            Some(r#"{"name":"fast","type":"max_latency_ms","config":"20000"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, report) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?run=r1", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 1);
    assert_eq!(report["runsSkipped"], 0);

    let (status, report) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions/evaluate?since=1970-01-01T00:00:00Z&until=2000-01-01T00:00:00Z",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 2);
}

#[tokio::test]
async fn evaluate_capped_reports_skips() {
    let fake = Fake::new();
    for (index, id) in ["r1", "r2", "r3", "r4", "r5"].iter().enumerate() {
        seed_run(&fake, id, 1000 + index as i64);
    }
    fake.lock().max_evaluate_runs = Some(2);
    let app = app(&fake);
    let (status, report) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions/evaluate?project=default&limit=10",
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 2);
    assert_eq!(report["runsSkipped"], 3);
    assert_eq!(report["runLimit"], 2);
}

#[tokio::test]
async fn evaluate_caller_limit_cannot_exceed_cap() {
    let fake = Fake::new();
    for (index, id) in ["r1", "r2", "r3"].iter().enumerate() {
        seed_run(&fake, id, 1000 + index as i64);
    }
    fake.lock().max_evaluate_runs = Some(2);
    let app = app(&fake);
    let (status, report) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?limit=1", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 1);
    assert_eq!(report["runsSkipped"], 2);

    let (status, report) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?limit=99", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["runsEvaluated"], 2);
    assert_eq!(report["runsSkipped"], 1);
}

fn judge_fake(models: &[&str]) -> Fake {
    let fake = Fake::new();
    fake.lock().judge_models = models.iter().map(|model| model.to_string()).collect();
    fake
}

#[tokio::test]
async fn evaluate_refuses_disallowed_judge_model() {
    let fake = judge_fake(&["gpt-4o-mini"]);
    seed_run(&fake, "r1", 1000);
    fake.create_assertion(Assertion {
        name: "helpful".into(),
        r#type: "llm_judge".into(),
        enabled: true,
        config: r#"{"prompt":"is it helpful?","model":"priciest-model-available"}"#.into(),
        ..Assertion::default()
    })
    .expect("seed assertion");
    let app = app(&fake);

    let (status, body) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?run=r1", None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().expect("error");
    assert!(error.contains("priciest-model-available"), "{error}");
    assert!(error.contains("gpt-4o-mini"), "{error}");
}

#[tokio::test]
async fn create_assertion_refuses_disallowed_judge_model() {
    let fake = judge_fake(&["gpt-4o-mini"]);
    let app = app(&fake);
    let (status, body) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions",
            Some(r#"{"name":"helpful","type":"llm_judge","config":"{\"prompt\":\"ok?\",\"model\":\"priciest-model-available\"}"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("priciest-model-available")
    );

    let (status, _) = send_json(
        &app,
        make_request(
            Method::POST,
            "/api/assertions",
            Some(r#"{"name":"helpful","type":"llm_judge","config":"{\"prompt\":\"ok?\",\"model\":\"gpt-4o-mini\"}"}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn evaluate_judge_call_cap_reported() {
    let fake = judge_fake(&["gpt-4o-mini"]);
    for (index, id) in ["r1", "r2", "r3"].iter().enumerate() {
        seed_run(&fake, id, 1000 + index as i64);
    }
    for name in ["helpful", "concise"] {
        fake.create_assertion(Assertion {
            name: name.into(),
            r#type: "llm_judge".into(),
            enabled: true,
            config: r#"{"prompt":"judge it","model":"gpt-4o-mini"}"#.into(),
            ..Assertion::default()
        })
        .expect("seed assertion");
    }
    fake.lock().max_judge_calls = Some(0);
    let app = app(&fake);
    let (status, report) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?limit=50", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["judgeCalls"], 0);
    assert_eq!(report["judgeCallsSkipped"], 6);
    assert_eq!(report["judgeCallLimit"], 0);
}

#[tokio::test]
async fn evaluate_unknown_run_is_not_found() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, _) = send_json(
        &app,
        make_request(Method::POST, "/api/assertions/evaluate?run=nope", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn evaluate_rejects_malformed_bounds() {
    let fake = Fake::new();
    seed_run(&fake, "r1", 1000);
    let app = app(&fake);
    for query in ["?since=yesterday", "?limit=0", "?limit=lots"] {
        let (status, _) = send_json(
            &app,
            make_request(
                Method::POST,
                &format!("/api/assertions/evaluate{query}"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
}

// ------------------------------------------------------------------ alerts

fn alert_json(name: &str, webhook: &str) -> String {
    serde_json::json!({
        "project": "default", "name": name, "type": "error_rate",
        "threshold": 0.5, "windowSecs": 3600, "webhookUrl": webhook,
    })
    .to_string()
}

#[tokio::test]
async fn create_alert_rejects_unsafe_webhook() {
    let fake = Fake::new();
    let app = app(&fake);
    for webhook in [
        "http://127.0.0.1:9999/hook",
        "http://[::1]:9999/hook",
        "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
        "http://10.0.0.5/hook",
        "http://[fd00::1]/hook",
        "hooks.slack.com/services/T000/B000/xxxx",
    ] {
        let (status, _, body) = send(
            &app,
            make_request(Method::POST, "/api/alerts", Some(&alert_json("a", webhook))),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "webhook {webhook}: {}",
            body_text(&body)
        );
    }
}

#[tokio::test]
async fn create_alert_accepts_public_webhook() {
    let fake = Fake::new();
    let app = app(&fake);
    let (status, _, body) = send(
        &app,
        make_request(
            Method::POST,
            "/api/alerts",
            Some(&alert_json("ok", "https://93.184.216.34/hook")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", body_text(&body));
}

#[tokio::test]
async fn create_alert_window_cap_is_a_boundary_not_a_clamp() {
    let fake = Fake::new();
    let app = app(&fake);
    let alert_at = |name: &str, window_secs: i64| {
        serde_json::json!({
            "project": "default", "name": name, "type": "error_rate",
            "threshold": 0.5, "windowSecs": window_secs,
            "webhookUrl": "https://93.184.216.34/hook",
        })
        .to_string()
    };

    // The cap itself is valid and stored unchanged, not clamped.
    let (status, _, body) = send(
        &app,
        make_request(
            Method::POST,
            "/api/alerts",
            Some(&alert_at("at-cap", crate::alerts::MAX_WINDOW_SECS)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", body_text(&body));
    let created: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(created["windowSecs"], crate::alerts::MAX_WINDOW_SECS);

    // One second over is invalid input: 400, and nothing is stored.
    let (status, _, body) = send(
        &app,
        make_request(
            Method::POST,
            "/api/alerts",
            Some(&alert_at("over-cap", crate::alerts::MAX_WINDOW_SECS + 1)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body_text(&body));
    let text = body_text(&body);
    assert!(text.contains("windowSecs"), "{text}");
    let (status, _, body) = send(&app, make_request(Method::GET, "/api/alerts", None)).await;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(list["alerts"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn list_alerts_redacts_webhook_secret() {
    const SECRET: &str = "SUPERSECRETTOKEN";
    let fake = Fake::new();
    fake.create_alert(&NewAlert {
        project: "default".into(),
        name: "leaky".into(),
        kind: "error_rate".into(),
        threshold: 0.5,
        window_secs: 3600,
        config: String::new(),
        webhook_url: format!("https://hooks.slack.com/services/T000/B000/{SECRET}"),
        settle_secs: 0,
    })
    .expect("alert");
    let app = app(&fake);
    let (status, _, body) = send(&app, make_request(Method::GET, "/api/alerts", None)).await;
    assert_eq!(status, StatusCode::OK);
    let text = body_text(&body);
    assert!(
        !text.contains(SECRET),
        "list leaked the webhook token: {text}"
    );
    let list: Value = serde_json::from_slice(&body).expect("json");
    let alerts = list["alerts"].as_array().expect("alerts");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["webhookUrl"], "https://hooks.slack.com/***");
}

#[tokio::test]
async fn create_alert_response_is_redacted() {
    const SECRET: &str = "SUPERSECRETTOKEN";
    let fake = Fake::new();
    let app = app(&fake);
    let (status, _, body) = send(
        &app,
        make_request(
            Method::POST,
            "/api/alerts",
            Some(&alert_json(
                "hook",
                &format!("https://discord.com/api/webhooks/1/{SECRET}"),
            )),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(!body_text(&body).contains(SECRET), "{}", body_text(&body));
    assert!(body_text(&body).contains("https://discord.com/***"));
}

// ------------------------------------------------------------------ hub / SSE

fn drain_tick(receiver: &mut broadcast::Receiver<()>) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(()) => return true,
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Empty)
            | Err(broadcast::error::TryRecvError::Closed) => return false,
        }
    }
}

#[tokio::test]
async fn hub_broadcast() {
    let hub = Hub::new();
    let mut a = hub.subscribe();
    let mut b = hub.subscribe();

    hub.broadcast();
    assert!(drain_tick(&mut a));
    assert!(drain_tick(&mut b));

    hub.broadcast();
    hub.broadcast();
    assert!(drain_tick(&mut a));
    assert!(!drain_tick(&mut a), "ticks should coalesce to one");

    drop(a);
    drop(b);
    hub.broadcast();
    assert!(hub.subscribe().try_recv().is_err());
}

#[tokio::test]
async fn hub_concurrent() {
    let hub = Hub::new();
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let hub = hub.clone();
        tasks.push(tokio::spawn(async move {
            let mut receiver = hub.subscribe();
            hub.broadcast();
            assert!(drain_tick(&mut receiver));
        }));
    }
    for _ in 0..50 {
        hub.broadcast();
    }
    for task in tasks {
        task.await.expect("task");
    }
}

// ------------------------------------------------------------------ errors

#[test]
fn store_errors_map_through_context() {
    let conflict = anyhow::Error::new(crate::store::Error::Conflict)
        .context("UNIQUE constraint failed: saved_views.name");
    assert!(matches!(
        super::backend::map_store_error(conflict),
        BackendError::Conflict
    ));
    let not_found = anyhow::Error::new(crate::store::Error::NotFound);
    assert!(matches!(
        super::backend::map_store_error(not_found),
        BackendError::NotFound
    ));
    let other = anyhow::anyhow!("disk on fire");
    assert!(matches!(
        super::backend::map_store_error(other),
        BackendError::Other(_)
    ));
}
