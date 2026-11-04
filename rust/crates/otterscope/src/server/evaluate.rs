//! Assertion evaluation: the ingest-time path and the bounded on-demand
//! backfill (ported from the original Go implementation; this lives here
//! because the ingest module stops at normalization).
//!
//! The on-demand call must be bounded: `?run=`, a `?since=`/`?until=` window,
//! or an explicit `?limit=`. A bare project scored every run in it, which was
//! unbounded CPU and, with a judge key configured, one paid model call per run.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};

use super::backend::{EvaluateOptions, EvaluateReport};
use crate::evals::{self, Assertion};
use crate::model::Status;
use crate::store::{Filter, Store};

/// Caps one on-demand call so a single request cannot pin a core for minutes.
/// `AtomicI64` so tests and embedders can lower it like the Go package vars.
pub static MAX_EVALUATE_RUNS: AtomicI64 = AtomicI64::new(500);
/// Caps paid judge calls in one on-demand call.
pub static MAX_EVALUATE_JUDGE_CALLS: AtomicI64 = AtomicI64::new(100);

pub const UNBOUNDED_EVALUATE: &str =
    "unbounded evaluate: pass run=<id>, a since/until window, or limit=<n>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Unbounded,
    Invalid(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::Unbounded => f.write_str(UNBOUNDED_EVALUATE),
            ValidationError::Invalid(message) => f.write_str(message),
        }
    }
}

/// The "no unbounded evaluate" rule. At least one bound kind is required; all
/// three may be combined.
pub fn validate(options: &EvaluateOptions) -> Result<(), ValidationError> {
    if !options.run_id.is_empty() {
        return Ok(());
    }
    if let (Some(since), Some(until)) = (options.since, options.until) {
        if until < since {
            return Err(ValidationError::Invalid(format!(
                "until ({}) is before since ({})",
                rfc3339(until),
                rfc3339(since)
            )));
        }
        return Ok(());
    }
    if options.since.is_some() || options.until.is_some() || options.limit > 0 {
        return Ok(());
    }
    Err(ValidationError::Unbounded)
}

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvalStats {
    pub runs: i64,
    pub judge_calls: i64,
    pub judge_skipped: i64,
}

/// Caps paid judge calls in one pass. `remaining < 0` means unlimited (the
/// ingest path, already bounded by batch size).
struct JudgeBudget {
    remaining: i64,
}

impl JudgeBudget {
    fn allow(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        if self.remaining > 0 {
            self.remaining -= 1;
        }
        true
    }
}

/// Score each completed run against its project's enabled assertions.
/// Deterministic assertions always run; `llm_judge` assertions run subject to
/// their `sampleRate`, or unconditionally when `judge_all` is set. Safe to
/// re-run: results upsert by `(project, run, assertion)`.
pub async fn evaluate_runs(
    store: &Store,
    judge: &evals::Endpoint,
    refs: &[(String, String)],
    judge_all: bool,
) -> anyhow::Result<EvalStats> {
    evaluate_runs_with_budget(
        store,
        judge,
        refs,
        judge_all,
        &mut JudgeBudget { remaining: -1 },
    )
    .await
}

async fn evaluate_runs_with_budget(
    store: &Store,
    judge: &evals::Endpoint,
    refs: &[(String, String)],
    judge_all: bool,
    budget: &mut JudgeBudget,
) -> anyhow::Result<EvalStats> {
    let mut stats = EvalStats::default();
    let mut cache: HashMap<String, Vec<Assertion>> = HashMap::new();
    for (ref_project, run_id) in refs {
        let project = if ref_project.is_empty() {
            "default"
        } else {
            ref_project.as_str()
        };
        let Some((run, steps)) = store.get_run_in_project(project, run_id)? else {
            return Err(anyhow::Error::new(crate::store::Error::NotFound));
        };
        if run.status == Status::Running {
            continue;
        }
        stats.runs += 1;
        if !cache.contains_key(project) {
            let enabled = store
                .list_assertions(project)?
                .into_iter()
                .filter(|assertion| assertion.enabled)
                .collect();
            cache.insert(project.to_string(), enabled);
        }
        let assertions = cache.get(project).cloned().unwrap_or_default();
        if assertions.is_empty() {
            continue;
        }
        let judged: HashSet<i64> = if judge_all {
            HashSet::new()
        } else {
            store
                .results_for_run(project, &run.id)
                .map(|results| {
                    results
                        .into_iter()
                        .map(|result| result.assertion_id)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut results = Vec::new();
        for assertion in &assertions {
            if assertion.r#type == "llm_judge" {
                if judged.contains(&assertion.id) {
                    continue;
                }
                if !judge_all && !sampled(assertion) {
                    continue;
                }
                let model_allowed = assertion
                    .judge_model()
                    .map(|model| judge.allows_model(&model))
                    .unwrap_or(false);
                if !judge.key.is_empty() && model_allowed {
                    if !budget.allow() {
                        stats.judge_skipped += 1;
                        continue;
                    }
                    stats.judge_calls += 1;
                }
                results.push(evals::judge(judge, assertion, &run, &steps).await);
                continue;
            }
            results.push(evals::evaluate(assertion, &run, &steps));
        }
        store.save_assertion_results(project, &run.id, &results)?;
    }
    Ok(stats)
}

fn sampled(assertion: &Assertion) -> bool {
    let Ok(config) = serde_json::from_str::<JudgeSample>(&assertion.config) else {
        return false;
    };
    if config.sample_rate <= 0.0 || config.sample_rate >= 1.0 {
        return true;
    }
    rand::random::<f64>() < config.sample_rate
}

#[derive(serde::Deserialize)]
struct JudgeSample {
    #[serde(default, rename = "sampleRate")]
    sample_rate: f64,
}

/// The effective per-call run cap: the server maximum, lowered by an explicit
/// `?limit=` the caller asked for.
pub fn run_cap(options: &EvaluateOptions) -> i64 {
    run_cap_with(MAX_EVALUATE_RUNS.load(Ordering::Relaxed), options)
}

fn run_cap_with(server_max: i64, options: &EvaluateOptions) -> i64 {
    let mut limit = server_max;
    if limit <= 0 {
        limit = 1;
    }
    if options.limit > 0 && options.limit < limit {
        limit = options.limit;
    }
    limit
}

/// Backfill assertion results over the runs the options select. Bounded by the
/// caller's bound, the run cap and the judge-call cap; what the caps skipped is
/// reported, never dropped.
pub async fn evaluate_project(
    store: &Store,
    judge: &evals::Endpoint,
    options: &EvaluateOptions,
) -> anyhow::Result<EvaluateReport> {
    let mut judge_cap = MAX_EVALUATE_JUDGE_CALLS.load(Ordering::Relaxed);
    if judge_cap < 0 {
        judge_cap = 0;
    }
    let mut report = EvaluateReport {
        run_limit: run_cap(options),
        judge_call_limit: judge_cap,
        ..EvaluateReport::default()
    };
    if let Err(err) = validate(options) {
        return Err(anyhow::anyhow!(err.to_string()));
    }
    let mut budget = JudgeBudget {
        remaining: report.judge_call_limit,
    };

    if !options.run_id.is_empty() {
        let project = if options.project.is_empty() {
            "default"
        } else {
            options.project.as_str()
        };
        let stats = evaluate_runs_with_budget(
            store,
            judge,
            &[(project.to_string(), options.run_id.clone())],
            true,
            &mut budget,
        )
        .await?;
        report.runs_evaluated = stats.runs;
        report.judge_calls = stats.judge_calls;
        report.judge_calls_skipped = stats.judge_skipped;
        return Ok(report);
    }

    let filter = Filter {
        project: options.project.clone(),
        since: options.since,
        until: options.until,
        ..Filter::default()
    };
    let candidates = completed_run_count(store, &filter)?;
    let refs = collect_run_refs(store, &filter, report.run_limit)?;
    let stats = evaluate_runs_with_budget(store, judge, &refs, true, &mut budget).await?;
    report.runs_evaluated = stats.runs;
    report.judge_calls = stats.judge_calls;
    report.judge_calls_skipped = stats.judge_skipped;
    let skipped = candidates - refs.len() as i64;
    if skipped > 0 {
        report.runs_skipped = skipped;
    }
    Ok(report)
}

/// Count the completed runs a window selects, so the caller can be told
/// exactly how many the cap left behind.
fn completed_run_count(store: &Store, filter: &Filter) -> anyhow::Result<i64> {
    let total = store.count_runs(filter)?;
    let running = store.count_runs(&Filter {
        project: filter.project.clone(),
        since: filter.since,
        until: filter.until,
        status: Status::Running.as_str().to_string(),
        ..Filter::default()
    })?;
    Ok(total - running.min(total))
}

/// Load at most `limit` completed runs, paging so a wide window costs a bounded
/// read instead of loading the whole table.
fn collect_run_refs(
    store: &Store,
    filter: &Filter,
    limit: i64,
) -> anyhow::Result<Vec<(String, String)>> {
    const PAGE: i64 = 200;
    let mut refs = Vec::new();
    let mut offset = 0;
    while (refs.len() as i64) < limit {
        let runs = store.list_runs(filter, PAGE, offset)?;
        if runs.is_empty() {
            break;
        }
        for run in &runs {
            if run.status == Status::Running {
                continue;
            }
            if refs.len() as i64 >= limit {
                break;
            }
            refs.push((run.project.clone(), run.id.clone()));
        }
        if runs.len() < PAGE as usize {
            break;
        }
        offset += PAGE;
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> EvaluateOptions {
        EvaluateOptions {
            project: "default".into(),
            ..EvaluateOptions::default()
        }
    }

    #[test]
    fn refuses_an_unbounded_call() {
        assert_eq!(validate(&options()), Err(ValidationError::Unbounded));
        let with_limit = EvaluateOptions {
            limit: 5,
            ..options()
        };
        assert_eq!(validate(&with_limit), Ok(()));
    }

    #[test]
    fn accepts_each_bound_kind() {
        let by_run = EvaluateOptions {
            run_id: "r1".into(),
            ..options()
        };
        assert_eq!(validate(&by_run), Ok(()));
        let mut by_window = options();
        by_window.since = Some(Utc::now());
        assert_eq!(validate(&by_window), Ok(()));
        let mut reversed = options();
        reversed.since = Some(Utc::now());
        reversed.until = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(matches!(
            validate(&reversed),
            Err(ValidationError::Invalid(_))
        ));
    }

    #[test]
    fn caller_limit_cannot_exceed_the_cap() {
        let by_default = run_cap_with(2, &options());
        let lowered = run_cap_with(
            2,
            &EvaluateOptions {
                limit: 1,
                ..options()
            },
        );
        let raised = run_cap_with(
            2,
            &EvaluateOptions {
                limit: 99,
                ..options()
            },
        );
        assert_eq!(by_default, 2);
        assert_eq!(lowered, 1);
        assert_eq!(raised, 2);
    }

    #[test]
    fn run_cap_is_at_least_one() {
        assert_eq!(run_cap_with(0, &options()), 1);
        assert_eq!(run_cap_with(-4, &options()), 1);
    }
}
