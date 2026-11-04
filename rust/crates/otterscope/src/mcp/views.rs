//! Compact, agent-friendly projections of the domain types. Kept separate from
//! the HTTP API's wire types so the two can evolve independently.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::model::{Message, Run, Step};
use crate::store::{AssertionRate, Stats};

fn timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn duration_ms(start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> i64 {
    end.map_or(0, |end| (end - start).num_milliseconds())
}

pub fn run_to_mcp(run: &Run) -> Value {
    let mut out = json!({
        "id": run.id,
        "project": run.project,
        "service": run.service,
        "agent": run.agent_name,
        "status": run.status.as_str(),
        "start": timestamp(run.start),
        "durationMs": duration_ms(run.start, run.end),
        "inputTokens": run.input_tokens,
        "outputTokens": run.output_tokens,
        "llmCalls": run.llm_calls,
        "toolCalls": run.tool_calls,
        "models": run.models,
    });
    if let Some(cost) = run.cost_usd {
        out["costUsd"] = json!(cost);
    }
    if !run.error.is_empty() {
        out["error"] = json!(run.error);
    }
    out
}

pub fn runs_to_mcp(runs: &[Run]) -> Vec<Value> {
    runs.iter().map(run_to_mcp).collect()
}

pub fn steps_to_mcp(steps: &[Step]) -> Vec<Value> {
    steps
        .iter()
        .map(|step| {
            let mut out = json!({
                "id": step.id,
                "parentId": step.parent_id,
                "kind": step.kind.as_str(),
                "name": step.name,
                "status": step.status.as_str(),
                "durationMs": duration_ms(step.start, step.end),
            });
            if !step.error.is_empty() {
                out["error"] = json!(step.error);
            }
            if let Some(llm) = &step.llm {
                let mut detail = json!({
                    "model": llm.request_model,
                    "provider": llm.provider,
                    "inputTokens": llm.input_tokens,
                    "outputTokens": llm.output_tokens,
                });
                if let Some(cost) = llm.cost_usd {
                    detail["costUsd"] = json!(cost);
                }
                if !llm.input_messages.is_empty() {
                    detail["inputMessages"] = messages(&llm.input_messages);
                }
                if !llm.output_messages.is_empty() {
                    detail["outputMessages"] = messages(&llm.output_messages);
                }
                out["llm"] = detail;
            }
            if let Some(tool) = &step.tool {
                let mut detail = json!({"name": tool.name});
                if !tool.arguments.is_empty() {
                    detail["arguments"] = json!(tool.arguments);
                }
                if !tool.result.is_empty() {
                    detail["result"] = json!(tool.result);
                }
                out["tool"] = detail;
            }
            out
        })
        .collect()
}

fn messages(messages: &[Message]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect(),
    )
}

/// Wire form of [`Stats`] for MCP: the store keeps snake_case Rust field
/// names, while MCP clients expect the API's camelCase names.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsView<'a> {
    runs: i64,
    errors: i64,
    avg_duration_ms: f64,
    p50_duration_ms: i64,
    p95_duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_cost_usd: Option<f64>,
    input_tokens: i64,
    output_tokens: i64,
    assertion_rates: Vec<AssertionRateView<'a>>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AssertionRateView<'a> {
    name: &'a str,
    passed: i64,
    total: i64,
}

pub fn stats_to_mcp(stats: &Stats) -> Value {
    let view = StatsView {
        runs: stats.runs,
        errors: stats.errors,
        avg_duration_ms: stats.avg_duration_ms,
        p50_duration_ms: stats.p50_duration_ms,
        p95_duration_ms: stats.p95_duration_ms,
        total_cost_usd: stats.total_cost_usd,
        avg_cost_usd: stats.avg_cost_usd,
        input_tokens: stats.input_tokens,
        output_tokens: stats.output_tokens,
        assertion_rates: stats
            .assertion_rates
            .iter()
            .map(assertion_rate_view)
            .collect(),
    };
    let Ok(mut value) = serde_json::to_value(view) else {
        return Value::Null;
    };
    // Emit null for an empty list to keep the wire shape stable.
    if stats.assertion_rates.is_empty() {
        value["assertionRates"] = Value::Null;
    }
    value
}

fn assertion_rate_view(rate: &AssertionRate) -> AssertionRateView<'_> {
    AssertionRateView {
        name: &rate.name,
        passed: rate.pass,
        total: rate.pass + rate.fail,
    }
}
