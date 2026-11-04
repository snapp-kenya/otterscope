//! Sequential, append-only migrations embedded in the binary.
//!
//! Schema changes only ever arrive as a new numbered file; an existing file is
//! never edited. A database migrated by a newer version cannot be opened by an
//! older one, which is why `open` snapshots the file before applying pending
//! migrations.

use rusqlite::Connection;

/// The migrations, in application order, embedded at build time.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_meta.sql", include_str!("../../migrations/0001_meta.sql")),
    ("0002_runs_steps.sql", include_str!("../../migrations/0002_runs_steps.sql")),
    ("0003_raw_batches.sql", include_str!("../../migrations/0003_raw_batches.sql")),
    ("0004_llm_token_details.sql", include_str!("../../migrations/0004_llm_token_details.sql")),
    ("0005_run_models.sql", include_str!("../../migrations/0005_run_models.sql")),
    ("0006_step_detail.sql", include_str!("../../migrations/0006_step_detail.sql")),
    ("0007_cost.sql", include_str!("../../migrations/0007_cost.sql")),
    ("0008_service_idx.sql", include_str!("../../migrations/0008_service_idx.sql")),
    ("0009_projects.sql", include_str!("../../migrations/0009_projects.sql")),
    ("0010_assertions.sql", include_str!("../../migrations/0010_assertions.sql")),
    ("0011_project_scoped_keys.sql", include_str!("../../migrations/0011_project_scoped_keys.sql")),
    ("0012_alerts.sql", include_str!("../../migrations/0012_alerts.sql")),
    ("0013_fts.sql", include_str!("../../migrations/0013_fts.sql")),
    ("0014_shares.sql", include_str!("../../migrations/0014_shares.sql")),
    ("0015_saved_views.sql", include_str!("../../migrations/0015_saved_views.sql")),
    ("0016_prompt.sql", include_str!("../../migrations/0016_prompt.sql")),
    ("0017_read_tokens.sql", include_str!("../../migrations/0017_read_tokens.sql")),
    ("0018_audit.sql", include_str!("../../migrations/0018_audit.sql")),
    ("0019_assertion_results_project.sql", include_str!("../../migrations/0019_assertion_results_project.sql")),
    ("0020_alert_settle.sql", include_str!("../../migrations/0020_alert_settle.sql")),
    ("0021_token_scopes.sql", include_str!("../../migrations/0021_token_scopes.sql")),
];

pub(crate) fn apply(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             name TEXT PRIMARY KEY,
             applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         )",
    )?;
    for (name, body) in MIGRATIONS {
        let done: i64 = conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
            [name],
            |r| r.get(0),
        )?;
        if done > 0 {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(body)?;
        tx.execute("INSERT INTO schema_migrations (name) VALUES (?1)", [name])?;
        tx.commit()?;
    }
    Ok(())
}
