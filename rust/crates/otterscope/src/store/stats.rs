//! Aggregate statistics over a filtered run set.

use rusqlite::params_from_iter;
use rusqlite::types::Value;

use super::runs::{escape_like, fts_query};
use super::{AssertionRate, Filter, Stats};

/// Build the shared WHERE clause for run filters. Zero values mean "no
/// constraint".
pub(crate) fn filter_where(filter: &Filter) -> (String, Vec<Value>) {
    let mut where_clause = String::from(" WHERE 1=1");
    let mut args = Vec::new();

    if !filter.project.is_empty() {
        where_clause.push_str(" AND project = ?");
        args.push(Value::Text(filter.project.clone()));
    }
    if !filter.status.is_empty() {
        where_clause.push_str(" AND status = ?");
        args.push(Value::Text(filter.status.clone()));
    }
    if !filter.service.is_empty() {
        where_clause.push_str(" AND service LIKE ? ESCAPE '\\'");
        args.push(Value::Text(format!("{}%", escape_like(&filter.service))));
    }
    if !filter.model.is_empty() {
        where_clause.push_str(" AND models LIKE ? ESCAPE '\\'");
        args.push(Value::Text(format!("%{}%", escape_like(&filter.model))));
    }
    if !filter.prompt.is_empty() {
        where_clause.push_str(" AND prompts LIKE ? ESCAPE '\\'");
        args.push(Value::Text(format!("%{}%", escape_like(&filter.prompt))));
    }
    let query = fts_query(&filter.query);
    if !query.is_empty() {
        where_clause.push_str(
            " AND EXISTS (SELECT 1 FROM steps_fts \
             JOIN steps ON steps.rowid = steps_fts.rowid \
             WHERE steps.project = runs.project AND steps.run_id = runs.id \
             AND steps_fts MATCH ?)",
        );
        args.push(Value::Text(query));
    }
    if let Some(since) = filter.since {
        where_clause.push_str(" AND start_ns >= ?");
        args.push(Value::Integer(super::nanos(since)));
    }
    if let Some(until) = filter.until {
        where_clause.push_str(" AND start_ns <= ?");
        args.push(Value::Integer(super::nanos(until)));
    }
    (where_clause, args)
}

/// Qualify filter columns for the joined stats query.
fn replace_runs_alias(where_clause: &str) -> String {
    let mut out = where_clause.to_string();
    for column in [
        "project", "status", "service", "models", "prompts", "start_ns",
    ] {
        out = out.replace(&format!(" AND {column} "), &format!(" AND runs.{column} "));
    }
    out
}

impl super::Store {
    /// Aggregate all runs matching `filter`, including per-assertion pass
    /// rates.
    pub fn get_stats(&self, filter: &Filter) -> anyhow::Result<Stats> {
        let (where_clause, args) = filter_where(filter);
        let conn = self.lock()?;

        let mut stats = Stats::default();
        {
            let sql = format!(
                "
                SELECT count(*),
                       coalesce(sum(status = 'error'), 0),
                       coalesce(avg((coalesce(end_ns, start_ns) - start_ns) / 1e6), 0),
                       sum(cost_usd), avg(cost_usd),
                       coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0)
                FROM runs{where_clause}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(args.iter()))?;
            if let Some(row) = rows.next()? {
                stats.runs = row.get(0)?;
                stats.errors = row.get(1)?;
                stats.avg_duration_ms = row.get(2)?;
                stats.total_cost_usd = row.get(3)?;
                stats.avg_cost_usd = row.get(4)?;
                stats.input_tokens = row.get(5)?;
                stats.output_tokens = row.get(6)?;
            }
        }

        if stats.runs > 0 {
            stats.p50_duration_ms =
                duration_percentile(&conn, &where_clause, &args, stats.runs, 50)?;
            stats.p95_duration_ms =
                duration_percentile(&conn, &where_clause, &args, stats.runs, 95)?;
        }

        {
            let aliased = replace_runs_alias(&where_clause);
            let sql = format!(
                "
                SELECT a.name, coalesce(sum(ar.pass), 0), count(*)
                FROM assertion_results ar
                JOIN assertions a ON a.id = ar.assertion_id
                JOIN runs ON runs.id = ar.run_id AND runs.project = ar.project{aliased}
                GROUP BY a.id ORDER BY a.id"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
                let pass: i64 = row.get(1)?;
                let total: i64 = row.get(2)?;
                Ok(AssertionRate {
                    name: row.get(0)?,
                    pass,
                    fail: total - pass,
                })
            })?;
            for row in rows {
                stats.assertion_rates.push(row?);
            }
        }

        Ok(stats)
    }
}

fn duration_percentile(
    conn: &rusqlite::Connection,
    where_clause: &str,
    args: &[Value],
    count: i64,
    percentile: i64,
) -> anyhow::Result<i64> {
    let mut offset = count * percentile / 100;
    if offset >= count {
        offset = count - 1;
    }
    let mut params = args.to_vec();
    params.push(Value::Integer(offset));
    let sql = format!(
        "SELECT (coalesce(end_ns, start_ns) - start_ns) / 1000000 FROM runs{where_clause} \
         ORDER BY (coalesce(end_ns, start_ns) - start_ns) LIMIT 1 OFFSET ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let ms: i64 = stmt.query_row(params_from_iter(params), |row| row.get(0))?;
    Ok(ms)
}
