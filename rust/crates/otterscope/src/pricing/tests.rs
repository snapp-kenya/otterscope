use super::*;
use crate::model::LlmCall;

fn almost(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn test_table() -> Table {
    let mut t = Table::empty();
    t.merge_json(
        br#"{
        "claude-sonnet-5": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75},
        "claude-sonnet-5-special": {"input": 6, "output": 30},
        "gpt-5": {"input": 1.25, "output": 10}
    }"#,
    )
    .unwrap();
    t
}

#[test]
fn longest_prefix_wins() {
    let tab = test_table();
    let rate = tab
        .lookup("claude-sonnet-5-special-20260101")
        .expect("special prefix");
    assert_eq!(rate.input, 6.0);

    let rate = tab
        .lookup("CLAUDE-SONNET-5-20260301")
        .expect("case-insensitive prefix");
    assert_eq!(rate.input, 3.0);
}

#[test]
fn unknown_model_no_cost() {
    assert!(
        test_table()
            .cost("mystery-model-9000", 1000, 100, 0, 0)
            .is_none()
    );
}

#[test]
fn cost_formula() {
    let tab = test_table();

    // 1M plain in @ $3 + 100k out @ $15 = 3 + 1.5.
    let usd = tab
        .cost("claude-sonnet-5", 1_000_000, 100_000, 0, 0)
        .unwrap();
    assert!(almost(usd, 4.5), "{usd}");

    // Cache read/write are subsets of input, billed at their own rates:
    // plain 200k @ $3 = 0.6, read 700k @ $0.3 = 0.21, write 100k @ $3.75
    // = 0.375.
    let usd = tab
        .cost("claude-sonnet-5", 1_000_000, 0, 700_000, 100_000)
        .unwrap();
    assert!(almost(usd, 0.6 + 0.21 + 0.375), "{usd}");

    // Providers without cache rates bill cache tokens at the input rate.
    let usd = tab.cost("gpt-5.2", 1_000_000, 0, 500_000, 0).unwrap();
    assert!(almost(usd, 1.25), "{usd}");
}

#[test]
fn cache_tokens_not_subset_of_input() {
    let tab = test_table();

    // input=100k (already net), cacheRead=700k. Additive:
    // 100k @ $3 + 700k @ $0.3 = 0.51.
    let usd = tab.cost("claude-sonnet-5", 100_000, 0, 700_000, 0).unwrap();
    assert!(
        almost(usd, (100_000.0 * 3.0 + 700_000.0 * 0.3) / 1e6),
        "{usd}"
    );
}

#[test]
fn default_table_non_empty() {
    let t = Table::default();
    assert!(t.lookup("claude-sonnet-5-20260301").is_some());
    assert!(t.lookup("claude-opus-5[1m]").is_some());
    assert!(t.lookup("gpt-5.6-sol-2026-01-01").is_some());
    assert!(t.lookup("mystery-model-9000").is_none());
}

#[test]
fn default_table_prices_claude_code_cache_additively() {
    let t = Table::default();
    // input_tokens excludes the cache tokens on a real Claude Code run.
    let usd = t.cost("claude-opus-5[1m]", 2, 110, 16_024, 15_181).unwrap();
    let want = (2.0 * 5.0 + 16_024.0 * 0.5 + 15_181.0 * 6.25 + 110.0 * 25.0) / 1e6;
    assert!(almost(usd, want), "{usd} != {want}");
}

#[test]
fn merge_overrides_replace_the_whole_rate() {
    let mut t = Table::default();
    t.merge_json(br#"{"claude-sonnet-5": {"input": 1, "output": 2}}"#)
        .unwrap();
    let rate = t.lookup("claude-sonnet-5-20260301").unwrap();
    assert_eq!(rate.input, 1.0);
    assert_eq!(rate.output, 2.0);
    assert_eq!(rate.cache_read, 0.0);
    assert_eq!(rate.cache_write, 0.0);
}

#[test]
fn merge_accepts_null_and_rejects_bad_json() {
    let mut t = test_table();
    t.merge_json(b"null").unwrap();
    assert!(t.lookup("claude-sonnet-5").is_some());

    let err = t.merge_json(b"{not json").unwrap_err().to_string();
    assert!(err.starts_with("pricing overrides: "), "{err}");
}

#[test]
fn merge_adds_unknown_models() {
    let mut t = Table::default();
    t.merge_json(
        br#"{"house-model": {"input": 4, "output": 8, "cacheRead": 0.4, "cacheWrite": 5}}"#,
    )
    .unwrap();
    let usd = t.cost("house-model-v2", 1_000_000, 0, 500_000, 0).unwrap();
    assert!(almost(usd, 2.0 + 0.2), "{usd}");
}

#[test]
fn call_cost_uses_response_model_then_request_model() {
    let tab = test_table();
    let mut call = LlmCall {
        request_model: "claude-sonnet-5".into(),
        input_tokens: 1_000_000,
        ..Default::default()
    };
    assert!(almost(cost(&tab, &call).unwrap(), 3.0));

    call.response_model = "gpt-5.2".into();
    assert!(almost(cost(&tab, &call).unwrap(), 1.25));

    call.response_model = "mystery-model-9000".into();
    assert!(cost(&tab, &call).is_none());
}

#[test]
fn call_cost_reasoning_is_a_subset_of_output() {
    let tab = test_table();
    let call = LlmCall {
        request_model: "claude-sonnet-5".into(),
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        reasoning_tokens: 50_000,
        ..Default::default()
    };
    let usd = cost(&tab, &call).unwrap();
    assert!(almost(usd, 4.5), "reasoning billed twice: {usd}");
}

#[test]
fn call_cost_needs_tokens_and_a_model() {
    let tab = test_table();
    assert!(cost(&tab, &LlmCall::default()).is_none());

    let no_model = LlmCall {
        input_tokens: 100,
        ..Default::default()
    };
    assert!(cost(&tab, &no_model).is_none());

    let zero = LlmCall {
        request_model: "claude-sonnet-5".into(),
        ..Default::default()
    };
    assert!(cost(&tab, &zero).is_none());
}
