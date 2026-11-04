//! Demo data seeding (port of `internal/sample`).
//!
//! Several services and models, tool loops, an error run, assertion results and
//! three alert rules — enough to make every screen meaningful on first launch.
//! The RNG is seeded so docs and screenshots stay stable.

use std::future::Future;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::alerts::Rule;
use crate::evals::{Assertion, Result as EvalResult};
use crate::model::{LlmCall, Message, Status, Step, StepKind, ToolCall};
use crate::pricing::{self, Table};

/// Store operations the seeder needs. Every real write goes through the store;
/// the trait keeps the seeded shape testable with a recording fake.
pub trait SeedStore: Send + Sync {
    fn create_assertion(
        &self,
        assertion: Assertion,
    ) -> impl Future<Output = Result<Assertion>> + Send;
    fn create_alert(&self, rule: Rule) -> impl Future<Output = Result<Rule>> + Send;
    fn set_alert_firing(&self, id: i64, firing: bool) -> impl Future<Output = Result<()>> + Send;
    fn upsert_steps(&self, steps: &[Step]) -> impl Future<Output = Result<()>> + Send;
    fn save_assertion_results(
        &self,
        project: &str,
        run_id: &str,
        results: &[EvalResult],
    ) -> impl Future<Output = Result<()>> + Send;
}

impl SeedStore for crate::store::Store {
    async fn create_assertion(&self, assertion: Assertion) -> Result<Assertion> {
        crate::store::Store::create_assertion(self, assertion)
    }

    async fn create_alert(&self, rule: Rule) -> Result<Rule> {
        crate::store::Store::create_alert(self, rule)
    }

    async fn set_alert_firing(&self, id: i64, firing: bool) -> Result<()> {
        crate::store::Store::set_alert_firing(self, id, firing)
    }

    async fn upsert_steps(&self, steps: &[Step]) -> Result<()> {
        crate::store::Store::upsert_steps(self, steps)
    }

    async fn save_assertion_results(
        &self,
        project: &str,
        run_id: &str,
        results: &[EvalResult],
    ) -> Result<()> {
        crate::store::Store::save_assertion_results(self, project, run_id, results)
    }
}

struct Flavor {
    service: &'static str,
    agent: &'static str,
    model: &'static str,
    provider: &'static str,
    tools: &'static [&'static str],
}

const FLAVORS: [Flavor; 3] = [
    Flavor {
        service: "support-bot",
        agent: "triage",
        model: "claude-sonnet-5",
        provider: "anthropic",
        tools: &["lookup_order", "search_kb", "escalate"],
    },
    Flavor {
        service: "research-agent",
        agent: "deep-research",
        model: "claude-fable-5",
        provider: "anthropic",
        tools: &["web_search", "fetch_page", "summarize_doc"],
    },
    Flavor {
        service: "checkout-assistant",
        agent: "cart-helper",
        model: "gpt-5.4-mini",
        provider: "openai",
        tools: &["get_cart", "apply_coupon"],
    },
];

/// Seeds `runs` sample runs spread over the past 7 days, two demo assertions
/// with results, and three alert rules — one already firing, so the alerts
/// screen shows both states rather than an empty table.
#[allow(clippy::default_constructed_unit_structs)] // pricing::Table is a unit struct in the scaffold
pub async fn seed<S: SeedStore>(store: &S, runs: usize) -> Result<()> {
    let mut rng = StdRng::seed_from_u64(42); // deterministic demo data
    let prices = Table::default();

    let fast = store
        .create_assertion(Assertion {
            project: "default".into(),
            name: "under-30s".into(),
            r#type: "max_latency_ms".into(),
            config: "30000".into(),
            enabled: true,
            ..Assertion::default()
        })
        .await
        .context("seed assertion under-30s")?;
    let polite = store
        .create_assertion(Assertion {
            project: "default".into(),
            name: "no-refusals".into(),
            r#type: "not_contains".into(),
            config: "I cannot help".into(),
            enabled: true,
            ..Assertion::default()
        })
        .await
        .context("seed assertion no-refusals")?;

    seed_alerts(store).await?;

    let now = Utc::now();
    for _ in 0..runs {
        let flavor = &FLAVORS[rng.random_range(0..FLAVORS.len())];
        let hours: i64 = rng.random_range(0..7 * 24);
        let seconds: i64 = rng.random_range(0..3600);
        let start = now - Duration::hours(hours) - Duration::seconds(seconds);
        let failed = rng.random::<f64>() < 0.12;

        let trace_id = format!("{:032x}", rng.random::<u64>());
        let steps = build_run(&mut rng, &trace_id, flavor, start, failed, &prices);
        let run_id = steps[0].run_id.clone();
        let project = steps[0].project.clone();
        store.upsert_steps(&steps).await?;

        let results = [
            EvalResult {
                assertion_id: fast.id,
                pass: rng.random::<f64>() > 0.1,
                ..EvalResult::default()
            },
            EvalResult {
                assertion_id: polite.id,
                pass: !failed || rng.random::<f64>() > 0.5,
                ..EvalResult::default()
            },
        ];
        store
            .save_assertion_results(&project, &run_id, &results)
            .await?;
    }
    Ok(())
}

fn build_run(
    rng: &mut StdRng,
    trace_id: &str,
    flavor: &Flavor,
    start: DateTime<Utc>,
    failed: bool,
    prices: &Table,
) -> Vec<Step> {
    let root_id = format!("{:016x}", rng.random::<u64>());
    let mut cursor = start;
    let mut steps = vec![Step {
        id: root_id.clone(),
        project: "default".into(),
        run_id: trace_id.to_string(),
        parent_id: String::new(),
        kind: StepKind::Agent,
        name: format!("invoke_agent {}", flavor.agent),
        service: flavor.service.into(),
        agent_name: flavor.agent.into(),
        status: Status::Ok,
        start,
        end: None,
        error: String::new(),
        llm: None,
        tool: None,
    }];

    let turns: i64 = 1 + rng.random_range(0..3);
    for turn in 0..turns {
        let input_tokens: i64 = 400 + rng.random_range(0..4000);
        let output_tokens: i64 = 50 + rng.random_range(0..800);
        let cache_read_tokens = (input_tokens as f64 * rng.random::<f64>() * 0.7) as i64;
        // Two prompt versions in the mix so the compare view has an axis.
        let prompt = format!(
            "{} v{}",
            flavor.agent,
            if rng.random::<f64>() < 0.5 { 3 } else { 2 }
        );
        let mut llm = LlmCall {
            provider: flavor.provider.into(),
            request_model: flavor.model.into(),
            response_model: flavor.model.into(),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            prompt,
            input_messages: vec![Message {
                role: "user".into(),
                content: sample_question(rng),
            }],
            output_messages: vec![Message {
                role: "assistant".into(),
                content: sample_answer(rng, turn == turns - 1, failed),
            }],
            ..LlmCall::default()
        };
        llm.cost_usd = pricing::cost(prices, &llm);
        let duration = Duration::milliseconds(800 + rng.random_range(0..4000));
        let llm_start = cursor + Duration::milliseconds(200);
        let llm_end = llm_start + duration;
        steps.push(Step {
            id: format!("{:016x}", rng.random::<u64>()),
            project: "default".into(),
            run_id: trace_id.to_string(),
            parent_id: root_id.clone(),
            kind: StepKind::Llm,
            name: format!("chat {}", flavor.model),
            service: flavor.service.into(),
            agent_name: String::new(),
            status: Status::Ok,
            start: llm_start,
            end: Some(llm_end),
            error: String::new(),
            llm: Some(llm),
            tool: None,
        });
        cursor = llm_end + Duration::milliseconds(400);

        // Failed runs must emit the final tool step that carries the error.
        if turn < turns - 1 || (failed && turn == turns - 1) || rng.random::<f64>() < 0.5 {
            let tool = flavor.tools[rng.random_range(0..flavor.tools.len())];
            let tool_duration = Duration::milliseconds(100 + rng.random_range(0..1500));
            let tool_start = cursor;
            let tool_end = tool_start + tool_duration;
            let (status, error) = if failed && turn == turns - 1 {
                (Status::Error, "tool timeout after 3 retries".to_string())
            } else {
                (Status::Ok, String::new())
            };
            steps.push(Step {
                id: format!("{:016x}", rng.random::<u64>()),
                project: "default".into(),
                run_id: trace_id.to_string(),
                parent_id: root_id.clone(),
                kind: StepKind::Tool,
                name: format!("execute_tool {tool}"),
                service: flavor.service.into(),
                agent_name: String::new(),
                status,
                start: tool_start,
                end: Some(tool_end),
                error: error.clone(),
                llm: None,
                tool: Some(ToolCall {
                    name: tool.into(),
                    call_id: format!("call_{:04x}", rng.random::<u32>()),
                    arguments: r#"{"query": "order A-1042"}"#.into(),
                    result: r#"{"status": "ok"}"#.into(),
                }),
            });
            cursor = tool_end + Duration::milliseconds(200);
        }
    }

    steps[0].end = Some(cursor + Duration::milliseconds(300));
    steps
}

fn sample_question(rng: &mut StdRng) -> String {
    const QUESTIONS: [&str; 4] = [
        "Where is my order A-1042? It was supposed to arrive Tuesday.",
        "Summarize the latest research on battery recycling.",
        "Can I apply two coupons to the same cart?",
        "My invoice shows a charge I don't recognize.",
    ];
    QUESTIONS[rng.random_range(0..QUESTIONS.len())].to_string()
}

fn sample_answer(rng: &mut StdRng, final_turn: bool, failed: bool) -> String {
    if failed && final_turn {
        return "I cannot help with that right now — an internal tool keeps timing out.".into();
    }
    if !final_turn {
        return "Let me look that up for you.".into();
    }
    const ANSWERS: [&str; 4] = [
        "Your order shipped yesterday and arrives tomorrow before noon. Tracking: 1Z-99XY.",
        "Three recent studies stand out; the 2026 Nature paper reports 94% lithium recovery.",
        "Only one coupon per cart, but COMBO10 covers both discounts — I've applied it.",
        "That charge is the annual plan renewal from March 3rd; I've emailed the invoice.",
    ];
    ANSWERS[rng.random_range(0..ANSWERS.len())].to_string()
}

/// Creates demo alert rules. The webhook URLs point at `example.invalid` so a
/// demo instance cannot post anywhere real, and one rule is left firing so the
/// badge and the settle column both have something to show.
async fn seed_alerts<S: SeedStore>(store: &S) -> Result<()> {
    let mut rules = vec![
        (
            Rule {
                project: "default".into(),
                name: "error-rate-over-10pct".into(),
                r#type: "error_rate".into(),
                threshold: 0.1,
                window_secs: 3600,
                settle_secs: 300,
                enabled: true,
                webhook_url: "https://hooks.example.invalid/demo".into(),
                ..Rule::default()
            },
            true,
        ),
        (
            Rule {
                project: "default".into(),
                name: "hourly-spend-over-5usd".into(),
                r#type: "cost".into(),
                threshold: 5.0,
                window_secs: 3600,
                settle_secs: 900,
                enabled: true,
                webhook_url: "https://hooks.example.invalid/demo".into(),
                ..Rule::default()
            },
            false,
        ),
        (
            Rule {
                project: "default".into(),
                name: "p95-over-20s".into(),
                r#type: "p95_latency".into(),
                threshold: 20_000.0,
                window_secs: 86_400,
                enabled: true,
                webhook_url: "https://hooks.example.invalid/demo".into(),
                ..Rule::default()
            },
            false,
        ),
    ];
    for (rule, firing) in rules.drain(..) {
        let name = rule.name.clone();
        let created = store
            .create_alert(rule)
            .await
            .with_context(|| format!("seed alert {name}"))?;
        if firing {
            store
                .set_alert_firing(created.id, true)
                .await
                .context("seed firing state")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    #[derive(Default)]
    struct RecordingStore {
        assertions: Mutex<Vec<Assertion>>,
        alerts: Mutex<Vec<Rule>>,
        firing: Mutex<Vec<(i64, bool)>>,
        steps: Mutex<Vec<Step>>,
        results: Mutex<Vec<(String, String, Vec<EvalResult>)>>,
        next_id: AtomicI64,
    }

    impl RecordingStore {
        fn next_id(&self) -> i64 {
            self.next_id.fetch_add(1, Ordering::SeqCst) + 1
        }

        fn steps(&self) -> Vec<Step> {
            self.steps.lock().expect("steps").clone()
        }
    }

    impl SeedStore for RecordingStore {
        async fn create_assertion(&self, mut assertion: Assertion) -> Result<Assertion> {
            assertion.id = self.next_id();
            self.assertions
                .lock()
                .expect("assertions")
                .push(assertion.clone());
            Ok(assertion)
        }

        async fn create_alert(&self, mut rule: Rule) -> Result<Rule> {
            rule.id = self.next_id();
            self.alerts.lock().expect("alerts").push(rule.clone());
            Ok(rule)
        }

        async fn set_alert_firing(&self, id: i64, firing: bool) -> Result<()> {
            self.firing.lock().expect("firing").push((id, firing));
            Ok(())
        }

        async fn upsert_steps(&self, steps: &[Step]) -> Result<()> {
            self.steps.lock().expect("steps").extend_from_slice(steps);
            Ok(())
        }

        async fn save_assertion_results(
            &self,
            project: &str,
            run_id: &str,
            results: &[EvalResult],
        ) -> Result<()> {
            self.results.lock().expect("results").push((
                project.to_string(),
                run_id.to_string(),
                results.to_vec(),
            ));
            Ok(())
        }
    }

    fn runs_of(steps: &[Step]) -> Vec<Vec<Step>> {
        let mut by_run: Vec<(String, Vec<Step>)> = Vec::new();
        for step in steps {
            match by_run.iter_mut().find(|(id, _)| *id == step.run_id) {
                Some((_, group)) => group.push(step.clone()),
                None => by_run.push((step.run_id.clone(), vec![step.clone()])),
            }
        }
        by_run.into_iter().map(|(_, group)| group).collect()
    }

    #[tokio::test]
    async fn seeds_assertions_alerts_and_runs() {
        let store = RecordingStore::default();
        seed(&store, 12).await.expect("seed");

        let assertions = store.assertions.lock().expect("assertions").clone();
        assert_eq!(assertions.len(), 2);
        assert_eq!(assertions[0].name, "under-30s");
        assert_eq!(assertions[0].r#type, "max_latency_ms");
        assert_eq!(assertions[1].name, "no-refusals");
        assert!(assertions.iter().all(|a| a.enabled));

        let alerts = store.alerts.lock().expect("alerts").clone();
        assert_eq!(alerts.len(), 3);
        assert_eq!(alerts[0].name, "error-rate-over-10pct");
        assert_eq!(alerts[0].settle_secs, 300);
        assert_eq!(alerts[1].name, "hourly-spend-over-5usd");
        assert_eq!(alerts[1].threshold, 5.0);
        assert_eq!(alerts[2].r#type, "p95_latency");
        assert_eq!(alerts[2].window_secs, 86_400);
        assert_eq!(
            store.firing.lock().expect("firing").as_slice(),
            &[(alerts[0].id, true)]
        );
        assert!(
            alerts
                .iter()
                .all(|a| a.webhook_url == "https://hooks.example.invalid/demo")
        );

        let steps = store.steps();
        let runs = runs_of(&steps);
        assert_eq!(runs.len(), 12);
        for run in &runs {
            assert_eq!(run[0].kind, StepKind::Agent);
            assert!(run[0].parent_id.is_empty());
            assert_eq!(run[0].project, "default");
            assert_eq!(run[0].run_id.len(), 32);
            assert!(run[0].end.is_some());
            assert!(run.iter().all(|step| step.project == "default"));
            assert!(run.iter().all(|step| step.run_id == run[0].run_id));
            assert!(run.iter().all(|step| step.id.len() == 16));
            for step in run.iter().skip(1) {
                assert_eq!(step.parent_id, run[0].id);
                assert!(step.start >= run[0].start);
            }
            assert!(run[0].end.expect("end") >= run[0].start);
        }

        let results = store.results.lock().expect("results").clone();
        assert_eq!(results.len(), 12);
        assert!(
            results
                .iter()
                .all(|(project, _, results)| project == "default" && results.len() == 2)
        );
    }

    #[tokio::test]
    async fn every_flavor_and_prompt_version_appears() {
        let store = RecordingStore::default();
        seed(&store, 40).await.expect("seed");
        let steps = store.steps();

        let services: HashSet<&str> = steps.iter().map(|s| s.service.as_str()).collect();
        for flavor in &FLAVORS {
            assert!(
                services.contains(flavor.service),
                "missing {}",
                flavor.service
            );
        }
        let models: HashSet<&str> = steps
            .iter()
            .filter_map(|s| s.llm.as_ref())
            .map(|llm| llm.request_model.as_str())
            .collect();
        for flavor in &FLAVORS {
            assert!(models.contains(flavor.model), "missing {}", flavor.model);
        }
        let prompts: HashSet<&str> = steps
            .iter()
            .filter_map(|s| s.llm.as_ref())
            .map(|llm| llm.prompt.as_str())
            .collect();
        assert!(prompts.contains("triage v2"));
        assert!(prompts.contains("triage v3"));
    }

    #[tokio::test]
    async fn failed_runs_carry_the_tool_error() {
        let store = RecordingStore::default();
        seed(&store, 40).await.expect("seed");
        let steps = store.steps();
        let error_steps: Vec<&Step> = steps.iter().filter(|s| !s.error.is_empty()).collect();
        assert!(!error_steps.is_empty(), "no failed sample run in 40 runs");
        for step in error_steps {
            assert_eq!(step.kind, StepKind::Tool);
            assert_eq!(step.status, Status::Error);
            assert_eq!(step.error, "tool timeout after 3 retries");
            assert!(!step.tool.as_ref().expect("tool").name.is_empty());
        }
        // A run with a failed step still has its full root spanning every step.
        for run in runs_of(&steps) {
            if run.iter().any(|s| s.status == Status::Error) {
                let root = &run[0];
                let root_end = root.end.expect("end");
                assert!(run.iter().all(|s| s.end.is_none_or(|end| end <= root_end)));
            }
        }
    }

    #[tokio::test]
    async fn timing_and_costs_are_plausible() {
        let store = RecordingStore::default();
        seed(&store, 20).await.expect("seed");
        let steps = store.steps();
        #[allow(clippy::default_constructed_unit_structs)] // see seed()
        let prices = Table::default();
        for step in &steps {
            let Some(end) = step.end else {
                panic!("step without end: {}", step.id);
            };
            let ms = (end - step.start).num_milliseconds();
            match step.kind {
                StepKind::Llm => {
                    assert!((800..=4800).contains(&ms), "llm duration {ms}");
                    let llm = step.llm.as_ref().expect("llm");
                    assert!((400..=4400).contains(&llm.input_tokens));
                    assert!((50..=850).contains(&llm.output_tokens));
                    assert_eq!(llm.cost_usd, pricing::cost(&prices, llm));
                }
                StepKind::Tool => assert!((100..=1600).contains(&ms), "tool duration {ms}"),
                StepKind::Agent => assert!(ms >= 0),
                StepKind::Generic => panic!("unexpected generic step"),
            }
        }
    }

    #[tokio::test]
    async fn run_starts_are_spread_over_the_last_week() {
        let before = Utc::now();
        let store = RecordingStore::default();
        seed(&store, 10).await.expect("seed");
        let steps = store.steps();
        for run in runs_of(&steps) {
            let start = run[0].start;
            assert!(start <= Utc::now());
            assert!(
                start >= before - Duration::days(7) - Duration::hours(1) - Duration::minutes(1)
            );
        }
    }

    #[tokio::test]
    async fn seeding_is_deterministic() {
        let first = RecordingStore::default();
        seed(&first, 8).await.expect("seed");
        let second = RecordingStore::default();
        seed(&second, 8).await.expect("seed");
        let shape = |steps: &[Step]| -> Vec<(String, String, StepKind, String, Status)> {
            steps
                .iter()
                .map(|s| {
                    (
                        s.id.clone(),
                        s.run_id.clone(),
                        s.kind,
                        s.name.clone(),
                        s.status,
                    )
                })
                .collect()
        };
        assert_eq!(shape(&first.steps()), shape(&second.steps()));
    }
}
