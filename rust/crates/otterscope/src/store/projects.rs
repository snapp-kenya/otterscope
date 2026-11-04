//! Projects: the isolation dimension and their ingest keys.

use anyhow::Context;
use chrono::Utc;
use rusqlite::{Connection, params};

use super::{Project, datetime, nanos, now_ns, random_hex};

/// Make sure a project row exists so a child row's foreign key can point at
/// it. Production resolves projects from ingest keys, so this is normally a
/// no-op; it keeps direct store use (tests, renormalization, a project named
/// by an assertion/alert before ingest starts) working without letting an
/// orphan child through.
pub(crate) fn ensure_project(conn: &Connection, name: &str) -> anyhow::Result<()> {
    if name == "default" {
        return Ok(());
    }
    // Read first: a named project is normally present, and ingest should not
    // attempt a write (and allocate a key) on every batch.
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM projects WHERE name = ?1)",
        params![name],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO projects (name, ingest_key, created_ns) VALUES (?,?,?)",
        params![name, random_hex(16)?, now_ns()],
    )?;
    Ok(())
}

impl super::Store {
    /// Add a project with a fresh random ingest key and return it.
    pub fn create_project(&self, name: &str) -> anyhow::Result<Project> {
        let project = Project {
            name: name.to_string(),
            ingest_key: random_hex(16)?,
            created_at: Utc::now(),
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO projects (name, ingest_key, created_ns) VALUES (?,?,?)",
            params![project.name, project.ingest_key, nanos(project.created_at)],
        )
        .with_context(|| format!("create project {name:?}"))?;
        super::audit::record(&conn, "create", "project", name);
        Ok(project)
    }

    /// All projects, oldest first.
    pub fn list_projects(&self) -> anyhow::Result<Vec<Project>> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT name, ingest_key, created_ns FROM projects ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            let created_ns: i64 = row.get(2)?;
            Ok(Project {
                name: row.get(0)?,
                ingest_key: row.get(1)?,
                created_at: datetime(created_ns),
            })
        })?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    /// Resolve an ingest key to its project name. `None` means the key is
    /// unknown; the empty key always resolves to "default".
    pub fn project_for_key(&self, key: &str) -> Option<String> {
        if key.is_empty() {
            return Some("default".to_string());
        }
        let conn = self.lock().ok()?;
        conn.query_row(
            "SELECT name FROM projects WHERE ingest_key = ?",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }
}
