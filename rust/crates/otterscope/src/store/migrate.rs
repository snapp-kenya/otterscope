//! Sequential, append-only migrations embedded in the binary.
//!
//! Schema changes only ever arrive as a new numbered file applied after the
//! baseline; an existing file is never edited. A database migrated by a newer
//! version cannot be opened by an older one, which is why `open` snapshots the
//! file before applying pending migrations.
//!
//! The baseline (`0001_initial.sql`) replaces the 21 migrations that were
//! carried over from the Go build. A database from that implementation is
//! refused rather than upgraded in place: see ADR-0008. The check is
//! deliberately conservative and runs before anything writes to the file.

use rusqlite::Connection;

use super::now_ns;

/// The migrations, in application order, embedded at build time.
pub const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_initial.sql",
    include_str!("../../migrations/0001_initial.sql"),
)];

/// What opening a database should do. `open` never guesses: an unrecognised
/// history is an error, not a half-applied migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// No user tables: apply every migration.
    Fresh,
    /// The known migrations are all applied.
    UpToDate,
    /// Some known migrations are not applied yet; snapshot first.
    Pending { applied: i64, pending: i64 },
    /// The database belongs to a different schema. The string is a ready-to-
    /// print explanation for the operator.
    Unsupported(String),
}

/// A database with an older history gets a snapshot before it is touched.
pub(crate) fn needs_snapshot(applied: i64, pending: i64) -> bool {
    applied > 0 && pending > 0
}

fn user_tables(conn: &Connection) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_schema
          WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut tables = Vec::new();
    for row in rows {
        tables.push(row?);
    }
    Ok(tables)
}

fn applied_names(conn: &Connection) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM schema_migrations ORDER BY name")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

fn refusal(detail: &str) -> Plan {
    Plan::Unsupported(format!(
        "this database cannot be opened by this build: {detail}. \
         Otterscope now starts from a new SQLite baseline (0001_initial.sql) and \
         does not migrate old databases in place. Start a new database (move the \
         old file aside, or point -db at a new path) and re-ingest your data; see \
         docs/adr/0008-sqlite-schema-redesign.md"
    ))
}

/// Decide what to do with the database without modifying it.
pub(crate) fn plan(conn: &Connection) -> anyhow::Result<Plan> {
    let tables = user_tables(conn)?;
    if tables.is_empty() {
        return Ok(Plan::Fresh);
    }
    if !tables.iter().any(|table| table == "schema_migrations") {
        return Ok(refusal(&format!(
            "it has tables but no migration history (found: {})",
            tables.join(", ")
        )));
    }

    let names = applied_names(conn)?;
    let unknown: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| !MIGRATIONS.iter().any(|(known, _)| known == name))
        .collect();
    if !unknown.is_empty() {
        return Ok(refusal(&format!(
            "its migration history belongs to a different schema (unknown migrations: {})",
            unknown.join(", ")
        )));
    }

    let pending = MIGRATIONS
        .iter()
        .filter(|(name, _)| !names.iter().any(|applied| applied == name))
        .count() as i64;
    if pending == 0 {
        return Ok(Plan::UpToDate);
    }

    // Only schema_migrations exists and it is empty: a baseline that was
    // interrupted before its first migration committed. Treat it as fresh.
    let has_data_tables = tables.iter().any(|table| table != "schema_migrations");
    if names.is_empty() && has_data_tables {
        return Ok(refusal(
            "it has tables but an empty migration history, so it was not created by this build",
        ));
    }

    Ok(Plan::Pending {
        applied: names.len() as i64,
        pending,
    })
}

/// Apply every pending migration in order, each in its own transaction.
pub(crate) fn apply(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             name       TEXT PRIMARY KEY,
             applied_ns INTEGER NOT NULL
         ) STRICT",
    )?;
    for (name, body) in MIGRATIONS {
        let done: i64 = conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        if done > 0 {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(body)?;
        tx.execute(
            "INSERT INTO schema_migrations (name, applied_ns) VALUES (?1, ?2)",
            rusqlite::params![name, now_ns()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_only_for_an_existing_database_with_pending_work() {
        assert!(!needs_snapshot(0, 1)); // fresh baseline
        assert!(needs_snapshot(1, 1)); // real upgrade
        assert!(!needs_snapshot(1, 0)); // already current
    }
}
