use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header::AUTHORIZATION},
    response::IntoResponse,
    routing::post,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use tokio::net::TcpListener;

use super::*;
use crate::model::{LlmCall, Message, Run, Status, Step, StepKind};

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn run_with_cost(cost_usd: Option<f64>, dur_ms: i64) -> Run {
    let start = ts(1000);
    Run {
        id: "run-1".into(),
        project: "default".into(),
        service: "svc".into(),
        agent_name: "agent".into(),
        status: Status::Ok,
        start,
        end: Some(start + Duration::milliseconds(dur_ms)),
        input_tokens: 0,
        output_tokens: 0,
        llm_calls: 0,
        tool_calls: 0,
        models: String::new(),
        prompts: String::new(),
        cost_usd,
        cost_partial: false,
        error: String::new(),
    }
}

fn run(cost_usd: f64, dur_ms: i64) -> Run {
    run_with_cost(Some(cost_usd), dur_ms)
}

fn step_with_llm(llm: LlmCall) -> Step {
    Step {
        id: "step-1".into(),
        project: "default".into(),
        run_id: "run-1".into(),
        parent_id: String::new(),
        kind: StepKind::Llm,
        name: "chat".into(),
        service: "svc".into(),
        agent_name: "agent".into(),
        status: Status::Ok,
        start: ts(1000),
        end: Some(ts(1001)),
        error: String::new(),
        llm: Some(llm),
        tool: None,
    }
}

fn steps_with_output(out: &str) -> Vec<Step> {
    vec![step_with_llm(LlmCall {
        output_messages: vec![Message {
            role: "assistant".into(),
            content: out.into(),
        }],
        ..Default::default()
    })]
}

fn assertion(kind: &str, config: &str) -> Assertion {
    Assertion {
        r#type: kind.into(),
        config: config.into(),
        ..Default::default()
    }
}

#[test]
fn evaluate_table() {
    let cases: Vec<(&str, Assertion, Run, Vec<Step>, bool)> = vec![
        (
            "contains pass",
            assertion("contains", "hello"),
            run(0.1, 100),
            steps_with_output("well hello there"),
            true,
        ),
        (
            "contains fail",
            assertion("contains", "hello"),
            run(0.1, 100),
            steps_with_output("goodbye"),
            false,
        ),
        (
            "not_contains pass",
            assertion("not_contains", "I cannot"),
            run(0.1, 100),
            steps_with_output("sure, done"),
            true,
        ),
        (
            "not_contains fail",
            assertion("not_contains", "I cannot"),
            run(0.1, 100),
            steps_with_output("I cannot help"),
            false,
        ),
        (
            "regex pass",
            assertion("regex", r"#\d+"),
            run(0.1, 100),
            steps_with_output("see ticket #42"),
            true,
        ),
        (
            "regex fail",
            assertion("regex", r"#\d+"),
            run(0.1, 100),
            steps_with_output("no ticket"),
            false,
        ),
        (
            "is_json pass",
            assertion("is_json", ""),
            run(0.1, 100),
            steps_with_output(r#"{"ok": true}"#),
            true,
        ),
        (
            "is_json fail",
            assertion("is_json", ""),
            run(0.1, 100),
            steps_with_output("not json"),
            false,
        ),
        (
            "latency pass",
            assertion("max_latency_ms", "500"),
            run(0.1, 100),
            Vec::new(),
            true,
        ),
        (
            "latency fail",
            assertion("max_latency_ms", "500"),
            run(0.1, 900),
            Vec::new(),
            false,
        ),
        (
            "cost pass",
            assertion("max_cost_usd", "0.5"),
            run(0.1, 100),
            Vec::new(),
            true,
        ),
        (
            "cost fail",
            assertion("max_cost_usd", "0.05"),
            run(0.1, 100),
            Vec::new(),
            false,
        ),
        (
            "multi-llm uses last output",
            assertion("contains", "final"),
            run(0.1, 100),
            steps_with_output("draft")
                .into_iter()
                .chain(steps_with_output("final answer"))
                .collect(),
            true,
        ),
    ];
    for (name, a, run, steps, pass) in cases {
        let got = evaluate(&a, &run, &steps);
        assert_eq!(got.pass, pass, "{name}: detail = {}", got.detail);
    }
}

#[test]
fn detail_strings_match_go() {
    let contains = evaluate(
        &assertion("contains", "hello"),
        &run(0.1, 100),
        &steps_with_output("goodbye"),
    );
    assert_eq!(contains.detail, r#"output does not contain "hello""#);

    let forbidden = evaluate(
        &assertion("not_contains", "I cannot"),
        &run(0.1, 100),
        &steps_with_output("I cannot help"),
    );
    assert_eq!(forbidden.detail, r#"output contains forbidden "I cannot""#);

    let regex = evaluate(
        &assertion("regex", r"#\d+"),
        &run(0.1, 100),
        &steps_with_output("no ticket"),
    );
    assert_eq!(regex.detail, r"output does not match /#\d+/");

    let invalid_regex = evaluate(
        &assertion("regex", "("),
        &run(0.1, 100),
        &steps_with_output("anything"),
    );
    assert_eq!(invalid_regex.detail, "invalid regex");

    let json = evaluate(
        &assertion("is_json", ""),
        &run(0.1, 100),
        &steps_with_output("not json"),
    );
    assert_eq!(json.detail, "output is not valid JSON");

    let latency = evaluate(&assertion("max_latency_ms", "500"), &run(0.1, 900), &[]);
    assert_eq!(latency.detail, "run took 900ms > 500ms");

    let cost = evaluate(&assertion("max_cost_usd", "0.05"), &run(0.1, 100), &[]);
    assert_eq!(cost.detail, "run cost $0.1000 > $0.0500");

    let unknown = evaluate(&assertion("nope", ""), &run(0.1, 100), &[]);
    assert_eq!(unknown.detail, r#"unknown type "nope""#);
}

#[test]
fn cost_unknown_fails() {
    let res = evaluate(
        &assertion("max_cost_usd", "1"),
        &run_with_cost(None, 1000),
        &[],
    );
    assert!(!res.pass);
    assert_eq!(res.detail, "run cost unknown (unpriced model)");
}

#[test]
fn evaluate_copies_assertion_identity() {
    let a = Assertion {
        id: 42,
        name: "helpful".into(),
        r#type: "contains".into(),
        config: "x".into(),
        ..Default::default()
    };
    let res = evaluate(&a, &run(0.0, 0), &steps_with_output("x"));
    assert_eq!(res.assertion_id, 42);
    assert_eq!(res.name, "helpful");
    assert_eq!(res.r#type, "contains");
}

#[test]
fn json_output_is_trimmed() {
    let res = evaluate(
        &assertion("is_json", ""),
        &run(0.0, 0),
        &steps_with_output("  {\"ok\":true}\n"),
    );
    assert!(res.pass);
}

#[test]
fn is_json_is_syntax_only_like_go() {
    let huge = evaluate(
        &assertion("is_json", ""),
        &run(0.0, 0),
        &steps_with_output("1e999"),
    );
    assert!(huge.pass, "{}", huge.detail);

    let nested = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    let deep = evaluate(
        &assertion("is_json", ""),
        &run(0.0, 0),
        &steps_with_output(&nested),
    );
    assert!(deep.pass, "{}", deep.detail);

    let trailing = evaluate(
        &assertion("is_json", ""),
        &run(0.0, 0),
        &steps_with_output("{} garbage"),
    );
    assert!(!trailing.pass);
}

#[test]
fn final_output_matches_go_joiner() {
    let llm = LlmCall {
        output_messages: vec![
            Message {
                role: "assistant".into(),
                content: String::new(),
            },
            Message {
                role: "assistant".into(),
                content: "visible".into(),
            },
        ],
        ..Default::default()
    };
    assert_eq!(final_output(&[step_with_llm(llm)]), "visible");
}

#[test]
fn absent_end_counts_as_zero_latency() {
    let mut r = run(0.0, 0);
    r.end = None;
    let res = evaluate(&assertion("max_latency_ms", "100"), &r, &[]);
    assert!(res.pass);
}

#[test]
fn validate_good_and_bad() {
    let good = [
        assertion("contains", "x"),
        assertion("not_contains", "x"),
        assertion("regex", r"\d+"),
        assertion("is_json", ""),
        assertion("max_latency_ms", "1000"),
        assertion("max_cost_usd", "0.5"),
        assertion(
            "llm_judge",
            r#"{"prompt":"grade it","model":"judge-model-1"}"#,
        ),
    ];
    for a in good {
        assert!(a.validate().is_ok(), "{} should be valid", a.r#type);
        assert!(validate(&a).is_ok());
    }

    let bad = [
        assertion("nope", ""),
        assertion("contains", ""),
        assertion("regex", "("),
        assertion("max_cost_usd", "-1"),
        assertion("max_latency_ms", "not a number"),
    ];
    for a in bad {
        assert!(a.validate().is_err(), "{} should be invalid", a.r#type);
    }

    let err = validate(&assertion("nope", "")).unwrap_err().to_string();
    assert_eq!(err, r#"unknown assertion type "nope""#);
    let err = validate(&assertion("contains", ""))
        .unwrap_err()
        .to_string();
    assert_eq!(err, "contains needs a non-empty config string");
    let err = validate(&assertion("max_cost_usd", "-1"))
        .unwrap_err()
        .to_string();
    assert_eq!(err, "max_cost_usd needs a positive number config");
    let err = validate(&assertion("regex", "(")).unwrap_err().to_string();
    assert!(err.starts_with("bad regex: "), "{err}");
}

#[test]
fn judge_model_only_for_llm_judge() {
    assert_eq!(assertion("contains", "x").judge_model().unwrap(), "");
    let judge = assertion(
        "llm_judge",
        r#"{"prompt":"grade it","model":"judge-model-1"}"#,
    );
    assert_eq!(judge.judge_model().unwrap(), "judge-model-1");
    assert!(assertion("llm_judge", "{}").judge_model().is_err());
}

#[test]
fn judge_config_validation_messages() {
    let missing = parse_judge_config("{}").unwrap_err().to_string();
    assert_eq!(missing, "llm_judge config needs prompt and model");

    let rate = parse_judge_config(r#"{"prompt":"p","model":"m","sampleRate":1.5}"#)
        .unwrap_err()
        .to_string();
    assert_eq!(rate, "sampleRate must be within 0..1");

    let negative = parse_judge_config(r#"{"prompt":"p","model":"m","sampleRate":-0.1}"#)
        .unwrap_err()
        .to_string();
    assert_eq!(negative, "sampleRate must be within 0..1");

    let unknown = parse_judge_config(r#"{"prompt":"p","model":"m","baseUrl":"http://evil"}"#)
        .unwrap_err()
        .to_string();
    assert!(
        unknown
            .starts_with("llm_judge config invalid (only prompt, model, sampleRate are allowed): "),
        "{unknown}"
    );

    let cfg = parse_judge_config(r#"{"prompt":"p","model":"m"}"#).unwrap();
    assert_eq!(cfg.sample_rate, 0.0);
    let cfg = parse_judge_config(r#"{"prompt":"p","model":"m","sampleRate":0.25}"#).unwrap();
    assert_eq!(cfg.sample_rate, 0.25);
}

#[test]
fn check_model() {
    let ep = Endpoint {
        models: vec!["GPT-4o-mini".into(), "o3-mini".into()],
        ..Default::default()
    };
    assert!(ep.allows_model(" gpt-4o-MINI "));
    assert!(ep.allows_model("O3-MINI"));
    assert!(!ep.allows_model("gpt-4o"));
    assert!(!Endpoint::default().allows_model("anything"));

    let err = Endpoint::default()
        .check_model("anything")
        .unwrap_err()
        .to_string();
    assert!(err.contains("OTTERSCOPE_JUDGE_MODELS"), "{err}");
}

#[derive(Clone, Debug, Default)]
struct SeenRequest {
    path: String,
    authorization: String,
    body: String,
}

#[derive(Clone)]
struct Stub {
    verdict: String,
    seen: Arc<Mutex<Option<SeenRequest>>>,
}

async fn chat_completions(
    State(stub): State<Stub>,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> Json<serde_json::Value> {
    *stub.seen.lock().unwrap() = Some(SeenRequest {
        path: uri.path().to_string(),
        authorization: headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body,
    });
    Json(json!({"choices":[{"message":{"content": stub.verdict}}]}))
}

async fn spawn_judge(verdict: &str) -> (String, Arc<Mutex<Option<SeenRequest>>>) {
    let seen = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/chat/completions", post(chat_completions))
        .with_state(Stub {
            verdict: verdict.into(),
            seen: seen.clone(),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

async fn rate_limited() -> impl IntoResponse {
    (StatusCode::TOO_MANY_REQUESTS, "rate limited")
}

async fn spawn_rate_limited() -> String {
    let app = Router::new().route("/chat/completions", post(rate_limited));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn endpoint(base_url: &str) -> Endpoint {
    Endpoint {
        base_url: base_url.into(),
        key: "sk-test".into(),
        models: vec!["judge-model-1".into()],
    }
}

fn judge_assertion() -> Assertion {
    Assertion {
        id: 7,
        name: "helpful".into(),
        r#type: "llm_judge".into(),
        config: serde_json::to_string(&JudgeConfig {
            prompt: "Is the answer helpful?".into(),
            model: "judge-model-1".into(),
            sample_rate: 0.0,
        })
        .unwrap(),
        ..Default::default()
    }
}

fn judge_run() -> (Run, Vec<Step>) {
    let step = step_with_llm(LlmCall {
        input_messages: vec![Message {
            role: "user".into(),
            content: "help me".into(),
        }],
        output_messages: vec![Message {
            role: "assistant".into(),
            content: "here is help".into(),
        }],
        ..Default::default()
    });
    (run(0.0, 1000), vec![step])
}

#[tokio::test]
async fn judge_pass_and_context() {
    let (base, seen) = spawn_judge("PASS — clear and helpful").await;
    let (run, steps) = judge_run();

    let res = judge(&endpoint(&base), &judge_assertion(), &run, &steps).await;
    assert!(res.pass, "{:?}", res);
    assert!(res.detail.contains("clear and helpful"));
    assert_eq!(res.assertion_id, 7);
    assert_eq!(res.name, "helpful");

    let seen = seen
        .lock()
        .unwrap()
        .clone()
        .expect("judge request captured");
    assert_eq!(seen.path, "/chat/completions");
    assert!(
        seen.authorization.starts_with("Bearer "),
        "{}",
        seen.authorization
    );
    assert!(seen.body.contains("help me"), "{}", seen.body);
    assert!(seen.body.contains("here is help"), "{}", seen.body);
    assert!(seen.body.contains("judge-model-1"), "{}", seen.body);
}

#[tokio::test]
async fn judge_fail() {
    let (base, _seen) = spawn_judge("FAIL: evasive answer").await;
    let (run, steps) = judge_run();

    let res = judge(&endpoint(&base), &judge_assertion(), &run, &steps).await;
    assert!(!res.pass);
    assert_eq!(res.detail, "FAIL: evasive answer");
}

#[tokio::test]
async fn judge_missing_key_skips() {
    let (run, steps) = judge_run();
    let res = judge(
        &Endpoint {
            base_url: "http://unused.invalid".into(),
            ..Default::default()
        },
        &judge_assertion(),
        &run,
        &steps,
    )
    .await;
    assert!(!res.pass);
    assert!(
        res.detail.contains("OTTERSCOPE_JUDGE_KEY"),
        "{}",
        res.detail
    );
}

#[tokio::test]
async fn judge_rejects_endpoint_in_config() {
    let a = Assertion {
        r#type: "llm_judge".into(),
        config: r#"{"prompt":"x","model":"m","baseUrl":"http://evil.tld","apiKeyEnv":"AWS_SECRET_ACCESS_KEY"}"#.into(),
        ..Default::default()
    };
    assert!(validate(&a).is_err());

    let (run, steps) = judge_run();
    let res = judge(&endpoint("http://unused.invalid"), &a, &run, &steps).await;
    assert!(!res.pass);
    assert!(!res.detail.is_empty());
    assert!(
        res.detail.starts_with("llm_judge config invalid"),
        "{}",
        res.detail
    );
}

#[tokio::test]
async fn judge_endpoint_error() {
    let base = spawn_rate_limited().await;
    let (run, steps) = judge_run();

    let res = judge(&endpoint(&base), &judge_assertion(), &run, &steps).await;
    assert!(!res.pass);
    assert!(res.detail.contains("429"), "{}", res.detail);
    assert!(res.detail.contains("rate limited"), "{}", res.detail);
}

#[tokio::test]
async fn judge_refuses_model_outside_allow_list() {
    let ep = Endpoint {
        models: vec!["allowed-model".into()],
        ..endpoint("http://unused.invalid")
    };
    let (run, steps) = judge_run();

    let res = judge(&ep, &judge_assertion(), &run, &steps).await;
    assert!(!res.pass);
    assert!(res.detail.contains("judge-model-1"), "{}", res.detail);
    assert!(res.detail.contains("allowed-model"), "{}", res.detail);
}

#[tokio::test]
async fn judge_unparseable_response() {
    async fn bad_json() -> impl IntoResponse {
        (StatusCode::OK, "not json")
    }
    let app = Router::new().route("/chat/completions", post(bad_json));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let (run, steps) = judge_run();
    let res = judge(
        &endpoint(&format!("http://{addr}")),
        &judge_assertion(),
        &run,
        &steps,
    )
    .await;
    assert!(!res.pass);
    assert_eq!(res.detail, "judge returned an unparseable response");
}

#[tokio::test]
async fn judge_missing_choices_is_unparseable() {
    async fn no_choices() -> impl IntoResponse {
        (StatusCode::OK, r#"{"choices":[]}"#)
    }
    let app = Router::new().route("/chat/completions", post(no_choices));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let (run, steps) = judge_run();
    let res = judge(
        &endpoint(&format!("http://{addr}")),
        &judge_assertion(),
        &run,
        &steps,
    )
    .await;
    assert!(!res.pass);
    assert_eq!(res.detail, "judge returned an unparseable response");
}
