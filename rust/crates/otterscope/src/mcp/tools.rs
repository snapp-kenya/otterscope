//! The read-only MCP tool set (port of `internal/mcp/tools.go`).
//!
//! Handlers are read-only and take decoded JSON arguments, returning the text
//! a model reads.

use anyhow::{Result, bail};
use chrono::{Duration, Utc};
use serde_json::{Map, Value, json};
use std::future::Future;

use super::views::{run_to_mcp, runs_to_mcp, stats_to_mcp, steps_to_mcp};
use crate::model::{Run, Step};
use crate::store::{Filter, Stats};

/// The store reads the tools depend on. A trait (rather than `Store` directly)
/// keeps the protocol testable without a database; the real store implements
/// it below.
pub trait McpStore: Send + Sync {
    fn list_runs(
        &self,
        filter: &Filter,
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = Result<Vec<Run>>> + Send;
    fn get_run(&self, id: &str) -> impl Future<Output = Result<Option<(Run, Vec<Step>)>>> + Send;
    fn get_stats(&self, filter: &Filter) -> impl Future<Output = Result<Stats>> + Send;
    fn list_assertions(
        &self,
        project: &str,
    ) -> impl Future<Output = Result<Vec<crate::evals::Assertion>>> + Send;
}

impl McpStore for crate::store::Store {
    async fn list_runs(&self, filter: &Filter, limit: i64, offset: i64) -> Result<Vec<Run>> {
        crate::store::Store::list_runs(self, filter, limit, offset)
    }

    async fn get_run(&self, id: &str) -> Result<Option<(Run, Vec<Step>)>> {
        crate::store::Store::get_run(self, id)
    }

    async fn get_stats(&self, filter: &Filter) -> Result<Stats> {
        crate::store::Store::get_stats(self, filter)
    }

    async fn list_assertions(&self, project: &str) -> Result<Vec<crate::evals::Assertion>> {
        crate::store::Store::list_assertions(self, project)
    }
}

/// One MCP tool: metadata plus its name. Handlers are dispatched by name in
/// [`run_tool`] so the registry stays a plain value.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Builds the tool set exposed over MCP.
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_runs",
            title: "List agent runs",
            description: "List recent agent runs, newest first. Optional filters: project, status (ok|error|running), model substring, service, and limit.",
            input_schema: object(
                &[
                    ("project", str_prop("Project name (default: all)")),
                    (
                        "status",
                        str_prop("Filter by run status: ok, error, or running"),
                    ),
                    ("model", str_prop("Substring match on model id")),
                    ("service", str_prop("Filter by service name")),
                    (
                        "q",
                        str_prop("Full-text search over message and tool content"),
                    ),
                    (
                        "limit",
                        int_prop("Max runs to return (default 20, max 200)"),
                    ),
                ],
                &[],
            ),
        },
        ToolDef {
            name: "get_run",
            title: "Get a run with its steps",
            description: "Fetch one run by id (trace id) with its full step tree, including LLM messages and tool arguments/results.",
            input_schema: object(&[("id", str_prop("Run id (trace id, hex)"))], &["id"]),
        },
        ToolDef {
            name: "get_stats",
            title: "Aggregate run statistics",
            description: "Aggregate stats over a filtered slice of runs: count, error rate, p50/p95 latency, cost, tokens, and assertion pass rates. Same filters as list_runs plus sinceMinutes.",
            input_schema: object(
                &[
                    ("project", str_prop("Project name (default: all)")),
                    ("status", str_prop("Filter by run status")),
                    ("model", str_prop("Substring match on model id")),
                    ("service", str_prop("Filter by service name")),
                    (
                        "sinceMinutes",
                        int_prop("Only runs started within the last N minutes"),
                    ),
                ],
                &[],
            ),
        },
        ToolDef {
            name: "list_assertions",
            title: "List eval assertions",
            description: "List the eval assertions configured for a project (contains, regex, latency/cost thresholds, llm_judge).",
            input_schema: object(&[("project", str_prop("Project name (default: all)"))], &[]),
        },
    ]
}

/// Runs one tool by name. Unknown names are a protocol error and are rejected
/// by the server before reaching here.
pub async fn run_tool<S: McpStore>(
    store: &S,
    name: &str,
    args: &Map<String, Value>,
) -> Result<String> {
    match name {
        "list_runs" => {
            let filter = Filter {
                project: arg_str(args, "project"),
                status: arg_str(args, "status"),
                model: arg_str(args, "model"),
                service: arg_str(args, "service"),
                query: arg_str(args, "q"),
                ..Filter::default()
            };
            let mut limit = arg_int(args, "limit", 20);
            if limit <= 0 || limit > 200 {
                limit = 20;
            }
            let runs = store.list_runs(&filter, limit, 0).await?;
            json_result(&json!({"runs": runs_to_mcp(&runs)}))
        }
        "get_run" => {
            let id = arg_str(args, "id");
            if id.is_empty() {
                bail!("id is required");
            }
            let Some((run, steps)) = store.get_run(&id).await? else {
                bail!("run {id:?} not found");
            };
            json_result(&json!({
                "run": run_to_mcp(&run),
                "steps": steps_to_mcp(&steps),
            }))
        }
        "get_stats" => {
            let since = arg_int(args, "sinceMinutes", 0);
            let filter = Filter {
                project: arg_str(args, "project"),
                status: arg_str(args, "status"),
                model: arg_str(args, "model"),
                service: arg_str(args, "service"),
                since: (since > 0).then(|| Utc::now() - Duration::minutes(since)),
                ..Filter::default()
            };
            let stats = store.get_stats(&filter).await?;
            json_result(&stats_to_mcp(&stats))
        }
        "list_assertions" => {
            let assertions = store.list_assertions(&arg_str(args, "project")).await?;
            json_result(&json!({"assertions": assertions}))
        }
        other => bail!("unknown tool: {other}"),
    }
}

fn arg_str(args: &Map<String, Value>, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn arg_int(args: &Map<String, Value>, key: &str, default: i64) -> i64 {
    match args.get(key) {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64))
            .unwrap_or(default),
        _ => default,
    }
}

fn json_result(value: &Value) -> Result<String> {
    serde_json::to_string_pretty(value).map_err(Into::into)
}

fn object(props: &[(&str, Value)], required: &[&str]) -> Value {
    let mut properties = Map::new();
    for (name, schema) in props {
        properties.insert((*name).to_string(), schema.clone());
    }
    let mut schema = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn str_prop(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn int_prop(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}
