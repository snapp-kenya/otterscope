//! Runs and steps: the write path's aggregate derivation, the read paths and
//! the full-text index that keeps them in sync.

use std::collections::HashSet;

use anyhow::Context;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::model::{LlmCall, Message, Run, Status, Step, StepKind, ToolCall};

use super::{Filter, datetime, nanos, sql_enum_error};

pub(crate) const RUN_COLUMNS: &str = "id, project, service, agent_name, status, start_ns, end_ns, \
     input_tokens, output_tokens, llm_calls, tool_calls, models, prompts, cost_usd, cost_partial, error";

const UPSERT_STEP_SQL: &str = "
    INSERT INTO steps
    (id, run_id, parent_id, kind, name, project, service, agent_name, status,
     start_ns, end_ns, error,
     provider, request_model, response_model, input_tokens, output_tokens,
     cache_read_tokens, cache_creation_tokens, reasoning_tokens,
     tool_name, tool_call_id, detail_json, cost_usd, prompt)
    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
    ON CONFLICT(project, id) DO UPDATE SET
     run_id=excluded.run_id, parent_id=excluded.parent_id,
     kind=excluded.kind, name=excluded.name, service=excluded.service,
     agent_name=excluded.agent_name, status=excluded.status,
     start_ns=excluded.start_ns, end_ns=excluded.end_ns, error=excluded.error,
     provider=excluded.provider, request_model=excluded.request_model,
     response_model=excluded.response_model, input_tokens=excluded.input_tokens,
     output_tokens=excluded.output_tokens,
     cache_read_tokens=excluded.cache_read_tokens,
     cache_creation_tokens=excluded.cache_creation_tokens,
     reasoning_tokens=excluded.reasoning_tokens,
     tool_name=excluded.tool_name, tool_call_id=excluded.tool_call_id,
     detail_json=excluded.detail_json, cost_usd=excluded.cost_usd, prompt=excluded.prompt
    WHERE steps.service = excluded.service";

/// A project name. The newtype keeps the two halves of [`RunKey`] from being
/// swapped at a call site that takes both as plain strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectName(String);

impl ProjectName {
    fn as_str(&self) -> &str {
        &self.0
    }
}

/// An external trace id. Newtype for the same reason as [`ProjectName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunId(String);

impl RunId {
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunKey {
    project: ProjectName,
    run_id: RunId,
}

impl super::Store {
    /// Write a batch of steps and re-derive the aggregates of every touched
    /// run in one transaction. Steps are keyed by (project, span ID), so
    /// re-delivered OTLP batches are naturally idempotent.
    pub fn upsert_steps(&self, steps: &[Step]) -> anyhow::Result<()> {
        if steps.is_empty() {
            return Ok(());
        }
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        upsert_steps_tx(&tx, steps)?;
        tx.commit()?;
        Ok(())
    }

    /// Returns runs newest-first, filtered and paged.
    pub fn list_runs(&self, filter: &Filter, limit: i64, offset: i64) -> anyhow::Result<Vec<Run>> {
        let (where_clause, mut args) = super::stats::filter_where(filter);
        args.push(Value::Integer(limit));
        args.push(Value::Integer(offset));
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs{where_clause} ORDER BY start_ns DESC LIMIT ? OFFSET ?"
        );
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), run_from_row)?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    /// Count runs matching `filter`, ignoring paging.
    pub fn count_runs(&self, filter: &Filter) -> anyhow::Result<i64> {
        let (where_clause, args) = super::stats::filter_where(filter);
        let sql = format!("SELECT count(*) FROM runs{where_clause}");
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt.query_row(params_from_iter(args), |row| row.get(0))?)
    }

    /// A bare trace ID can exist in more than one project; newest wins.
    pub fn get_run(&self, id: &str) -> anyhow::Result<Option<(Run, Vec<Step>)>> {
        self.get_run_where(
            "WHERE id = ? ORDER BY start_ns DESC LIMIT 1",
            vec![Value::Text(id.to_string())],
        )
    }

    /// A run scoped to an exact project, so a shared token cannot surface
    /// another project's run that happens to share a trace id.
    pub fn get_run_in_project(
        &self,
        project: &str,
        id: &str,
    ) -> anyhow::Result<Option<(Run, Vec<Step>)>> {
        self.get_run_where(
            "WHERE project = ? AND id = ?",
            vec![
                Value::Text(project.to_string()),
                Value::Text(id.to_string()),
            ],
        )
    }

    fn get_run_where(
        &self,
        where_clause: &str,
        args: Vec<Value>,
    ) -> anyhow::Result<Option<(Run, Vec<Step>)>> {
        let sql = format!("SELECT {RUN_COLUMNS} FROM runs {where_clause}");
        let conn = self.lock()?;
        let run = conn
            .query_row(&sql, params_from_iter(args), run_from_row)
            .optional()?;
        let Some(run) = run else {
            return Ok(None);
        };
        let steps = load_steps(&conn, &run.project, &run.id)?;
        Ok(Some((run, steps)))
    }
}

fn run_from_row(row: &Row<'_>) -> rusqlite::Result<Run> {
    let status_raw: String = row.get(4)?;
    let status = Status::try_from(status_raw.as_str()).map_err(|err| sql_enum_error(4, err))?;
    let start_ns: i64 = row.get(5)?;
    let end_ns: Option<i64> = row.get(6)?;
    Ok(Run {
        id: row.get(0)?,
        project: row.get(1)?,
        service: row.get(2)?,
        agent_name: row.get(3)?,
        status,
        start: datetime(start_ns),
        end: end_ns.map(datetime),
        input_tokens: row.get(7)?,
        output_tokens: row.get(8)?,
        llm_calls: row.get(9)?,
        tool_calls: row.get(10)?,
        models: row.get(11)?,
        prompts: row.get(12)?,
        cost_usd: row.get(13)?,
        cost_partial: row.get(14)?,
        error: row.get(15)?,
    })
}

/// The transactional body of [`super::Store::upsert_steps`], shared with
/// ingest. Returns the number of steps refused because the stored row under
/// that (project, span ID) belongs to another resource service.
///
/// The steps' foreign key to `runs` is deferred, so a new run may be created
/// by [`rederive_runs`] after its steps have been written, all in the same
/// transaction. The FTS index is maintained by SQLite triggers on `steps`.
pub(crate) fn upsert_steps_tx(conn: &Connection, steps: &[Step]) -> anyhow::Result<usize> {
    if steps.is_empty() {
        return Ok(0);
    }
    let empty_llm = LlmCall::default();
    let empty_tool = ToolCall::default();
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    let rejected;

    {
        let mut ins = conn.prepare(UPSERT_STEP_SQL)?;
        let mut count = 0usize;

        for step in steps {
            let project = if step.project.is_empty() {
                "default"
            } else {
                step.project.as_str()
            };
            let llm = step.llm.as_ref().unwrap_or(&empty_llm);
            let tool = step.tool.as_ref().unwrap_or(&empty_tool);
            let changed = ins
                .execute(params![
                    step.id,
                    step.run_id,
                    step.parent_id,
                    step.kind.as_str(),
                    step.name,
                    project,
                    step.service,
                    step.agent_name,
                    step.status.as_str(),
                    nanos(step.start),
                    step.end.map(nanos),
                    step.error,
                    llm.provider,
                    llm.request_model,
                    llm.response_model,
                    llm.input_tokens,
                    llm.output_tokens,
                    llm.cache_read_tokens,
                    llm.cache_creation_tokens,
                    llm.reasoning_tokens,
                    tool.name,
                    tool.call_id,
                    marshal_detail(step),
                    llm.cost_usd,
                    llm.prompt,
                ])
                .with_context(|| format!("insert step {}", step.id))?;
            // The guarded upsert writes nothing when the stored row belongs to
            // another service; SQLite reports that as zero affected rows.
            if changed == 0 {
                count += 1;
                continue;
            }

            let key = RunKey {
                project: ProjectName(project.to_string()),
                run_id: RunId(step.run_id.clone()),
            };
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
        rejected = count;
    }

    rederive_runs(conn, &keys)?;
    Ok(rejected)
}

struct DerivedRun {
    project: String,
    run_id: String,
    service: String,
    agent: String,
    error: String,
    models: String,
    prompts: String,
    start_ns: i64,
    end_ns: Option<i64>,
    input_tokens: i64,
    output_tokens: i64,
    llm_calls: i64,
    tool_calls: i64,
    has_error: i64,
    has_root: i64,
    cost: Option<f64>,
    partial: i64,
}

/// Recompute run rows entirely from their steps, making aggregates
/// structurally idempotent instead of bookkept. One grouped query derives
/// every touched run, so a batch of N trace ids does not cost N*7 statements.
fn rederive_runs(conn: &Connection, keys: &[RunKey]) -> anyhow::Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    // A temp table keeps the GROUP BY bounded by the touched runs without
    // splatting thousands of parameters into the statement.
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS touched_runs (
             project TEXT NOT NULL, run_id TEXT NOT NULL, PRIMARY KEY (project, run_id)) STRICT",
    )?;
    conn.execute_batch("DELETE FROM touched_runs")?;
    {
        let mut touch =
            conn.prepare("INSERT OR IGNORE INTO touched_runs (project, run_id) VALUES (?, ?)")?;
        for key in keys {
            touch.execute(params![key.project.as_str(), key.run_id.as_str()])?;
        }
    }

    let derived = {
        let mut stmt = conn.prepare(
            "
            SELECT t.project, t.run_id,
                   min(s.start_ns), max(s.end_ns),
                   sum(s.input_tokens), sum(s.output_tokens),
                   sum(s.kind = 'llm'), sum(s.kind = 'tool'),
                   max(s.error != ''), max(s.parent_id = ''),
                   coalesce((SELECT x.service FROM steps x
                              WHERE x.project = t.project AND x.run_id = t.run_id AND x.service != ''
                              ORDER BY x.start_ns LIMIT 1), ''),
                   coalesce((SELECT x.agent_name FROM steps x
                              WHERE x.project = t.project AND x.run_id = t.run_id AND x.agent_name != ''
                              ORDER BY x.start_ns LIMIT 1), ''),
                   coalesce((SELECT x.error FROM steps x
                              WHERE x.project = t.project AND x.run_id = t.run_id AND x.error != ''
                              ORDER BY x.start_ns LIMIT 1), ''),
                   coalesce(group_concat(DISTINCT CASE WHEN s.request_model != '' THEN s.request_model END), ''),
                   coalesce(group_concat(DISTINCT CASE WHEN s.prompt != '' THEN s.prompt END), ''),
                   sum(s.cost_usd),
                   max(s.kind = 'llm' AND s.cost_usd IS NULL AND (s.input_tokens > 0 OR s.output_tokens > 0))
            FROM touched_runs t
            JOIN steps s ON s.project = t.project AND s.run_id = t.run_id
            GROUP BY t.project, t.run_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DerivedRun {
                project: row.get(0)?,
                run_id: row.get(1)?,
                start_ns: row.get(2)?,
                end_ns: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                llm_calls: row.get(6)?,
                tool_calls: row.get(7)?,
                has_error: row.get(8)?,
                has_root: row.get(9)?,
                service: row.get(10)?,
                agent: row.get(11)?,
                error: row.get(12)?,
                models: row.get(13)?,
                prompts: row.get(14)?,
                cost: row.get(15)?,
                partial: row.get(16)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out
    };

    {
        let mut upsert = conn.prepare(
            "
            INSERT INTO runs (project, id, service, agent_name, status, start_ns, end_ns,
                              input_tokens, output_tokens, llm_calls, tool_calls, models, prompts,
                              cost_usd, cost_partial, error)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT(project, id) DO UPDATE SET
              service=excluded.service, agent_name=excluded.agent_name,
              status=excluded.status, start_ns=excluded.start_ns, end_ns=excluded.end_ns,
              input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens,
              llm_calls=excluded.llm_calls, tool_calls=excluded.tool_calls,
              models=excluded.models, prompts=excluded.prompts, cost_usd=excluded.cost_usd,
              cost_partial=excluded.cost_partial, error=excluded.error",
        )?;
        let mut ensured_projects = HashSet::new();
        for run in &derived {
            // Error outranks a delivered root, which outranks still-running.
            let status = if run.has_error != 0 {
                Status::Error
            } else if run.has_root != 0 {
                Status::Ok
            } else {
                Status::Running
            };
            if ensured_projects.insert(run.project.as_str()) {
                super::projects::ensure_project(conn, &run.project)?;
            }
            upsert
                .execute(params![
                    run.project,
                    run.run_id,
                    run.service,
                    run.agent,
                    status.as_str(),
                    run.start_ns,
                    run.end_ns,
                    run.input_tokens,
                    run.output_tokens,
                    run.llm_calls,
                    run.tool_calls,
                    run.models,
                    run.prompts,
                    run.cost,
                    run.partial != 0,
                    run.error,
                ])
                .with_context(|| format!("rederive run {}/{}", run.project, run.run_id))?;
        }
    }
    Ok(())
}

fn load_steps(conn: &Connection, project: &str, run_id: &str) -> anyhow::Result<Vec<Step>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, run_id, parent_id, kind, name, service, agent_name, status,
               start_ns, end_ns, error,
               provider, request_model, response_model, input_tokens, output_tokens,
               cache_read_tokens, cache_creation_tokens, reasoning_tokens,
               tool_name, tool_call_id, detail_json, cost_usd, prompt
        FROM steps WHERE project = ? AND run_id = ? ORDER BY start_ns",
    )?;
    let rows = stmt.query_map(params![project, run_id], |row| {
        let kind_raw: String = row.get(3)?;
        let kind = StepKind::try_from(kind_raw.as_str()).map_err(|err| sql_enum_error(3, err))?;
        let status_raw: String = row.get(7)?;
        let status = Status::try_from(status_raw.as_str()).map_err(|err| sql_enum_error(7, err))?;
        let start_ns: i64 = row.get(8)?;
        let end_ns: Option<i64> = row.get(9)?;
        let detail: String = row.get(21)?;
        let mut step = Step {
            id: row.get(0)?,
            project: project.to_string(),
            run_id: row.get(1)?,
            parent_id: row.get(2)?,
            kind,
            name: row.get(4)?,
            service: row.get(5)?,
            agent_name: row.get(6)?,
            status,
            start: datetime(start_ns),
            end: end_ns.map(datetime),
            error: row.get(10)?,
            llm: None,
            tool: None,
        };
        if kind == StepKind::Llm {
            step.llm = Some(LlmCall {
                provider: row.get(11)?,
                request_model: row.get(12)?,
                response_model: row.get(13)?,
                input_tokens: row.get(14)?,
                output_tokens: row.get(15)?,
                cache_read_tokens: row.get(16)?,
                cache_creation_tokens: row.get(17)?,
                reasoning_tokens: row.get(18)?,
                cost_usd: row.get(22)?,
                prompt: row.get(23)?,
                ..LlmCall::default()
            });
        }
        if kind == StepKind::Tool {
            step.tool = Some(ToolCall {
                name: row.get(19)?,
                call_id: row.get(20)?,
                ..ToolCall::default()
            });
        }
        apply_detail(&mut step, &detail);
        Ok(step)
    })?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row?);
    }
    Ok(steps)
}

/// Neutralize LIKE wildcards in user input.
pub(crate) fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Turn free text into a safe FTS5 MATCH expression: each whitespace term
/// becomes a quoted token (neutralizing operators and punctuation), AND-ed
/// together. Returns "" when there is nothing to match.
pub(crate) fn fts_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    terms.join(" ")
}

/// The JSON shape of the steps.detail_json column: kind-specific payloads too
/// large or nested for flat columns.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StepDetail {
    #[serde(
        rename = "inputMessages",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    input_messages: Vec<Message>,
    #[serde(
        rename = "outputMessages",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    output_messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    arguments: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    result: String,
}

fn marshal_detail(step: &Step) -> String {
    let mut detail = StepDetail::default();
    if let Some(llm) = &step.llm {
        detail.input_messages = llm.input_messages.clone();
        detail.output_messages = llm.output_messages.clone();
    }
    if let Some(tool) = &step.tool {
        detail.arguments = tool.arguments.clone();
        detail.result = tool.result.clone();
    }
    if detail.input_messages.is_empty()
        && detail.output_messages.is_empty()
        && detail.arguments.is_empty()
        && detail.result.is_empty()
    {
        return String::new();
    }
    serde_json::to_string(&detail).unwrap_or_default()
}

fn apply_detail(step: &mut Step, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let Ok(detail) = serde_json::from_str::<StepDetail>(raw) else {
        return;
    };
    if let Some(llm) = step.llm.as_mut() {
        llm.input_messages = detail.input_messages;
        llm.output_messages = detail.output_messages;
    }
    if let Some(tool) = step.tool.as_mut() {
        tool.arguments = detail.arguments;
        tool.result = detail.result;
    }
}
