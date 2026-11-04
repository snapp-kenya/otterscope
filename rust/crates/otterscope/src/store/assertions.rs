//! Assertions and their per-run verdicts.

use rusqlite::params;

use crate::evals::{Assertion, AssertionKind, Result as EvalResult};

use super::now_ns;

impl super::Store {
    /// Store a new assertion (already validated by the caller). The type is
    /// decoded through [`AssertionKind`] so an unknown value is refused here
    /// as well as by the schema's CHECK constraint.
    pub fn create_assertion(&self, mut assertion: Assertion) -> anyhow::Result<Assertion> {
        if assertion.project.is_empty() {
            assertion.project = "default".to_string();
        }
        let kind = assertion.kind().map_err(|err| anyhow::anyhow!("{err}"))?;
        let conn = self.lock()?;
        super::projects::ensure_project(&conn, &assertion.project)?;
        conn.execute(
            "INSERT INTO assertions (project, name, type, config, enabled, created_ns)
             VALUES (?,?,?,?,?,?)",
            params![
                assertion.project,
                assertion.name,
                kind.as_str(),
                assertion.config,
                assertion.enabled,
                now_ns()
            ],
        )
        .map_err(super::classify_write)?;
        assertion.id = conn.last_insert_rowid();
        super::audit::record(&conn, "create", "assertion", &assertion.name);
        Ok(assertion)
    }

    /// Assertions for one project, or every project when `project` is empty.
    pub fn list_assertions(&self, project: &str) -> anyhow::Result<Vec<Assertion>> {
        let conn = self.lock()?;
        let sql = if project.is_empty() {
            "SELECT id, project, name, type, config, enabled FROM assertions ORDER BY id"
                .to_string()
        } else {
            "SELECT id, project, name, type, config, enabled FROM assertions WHERE project = ? ORDER BY id"
                .to_string()
        };
        let mut stmt = conn.prepare(&sql)?;
        let map = |row: &rusqlite::Row<'_>| {
            let raw_type: String = row.get(3)?;
            let kind = AssertionKind::try_from(raw_type.as_str())
                .map_err(|err| super::sql_enum_error(3, err))?;
            Ok(Assertion {
                id: row.get(0)?,
                project: row.get(1)?,
                name: row.get(2)?,
                r#type: kind.as_str().to_string(),
                config: row.get(4)?,
                enabled: row.get(5)?,
            })
        };
        let rows = if project.is_empty() {
            stmt.query_map([], map)?
        } else {
            stmt.query_map(params![project], map)?
        };
        let mut assertions = Vec::new();
        for row in rows {
            assertions.push(row?);
        }
        Ok(assertions)
    }

    /// Remove an assertion. Its results go with it through the foreign key's
    /// ON DELETE CASCADE. False means there was nothing to remove.
    pub fn delete_assertion(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock()?;
        if conn.execute("DELETE FROM assertions WHERE id = ?", params![id])? == 0 {
            return Ok(false);
        }
        super::audit::record(&conn, "delete", "assertion", &id.to_string());
        Ok(true)
    }

    /// Upsert results for one run in one project. Project is required: a trace
    /// id alone does not identify a run, and the foreign key refuses a result
    /// for a run or assertion that does not exist.
    pub fn save_assertion_results(
        &self,
        project: &str,
        run_id: &str,
        results: &[EvalResult],
    ) -> anyhow::Result<()> {
        if results.is_empty() {
            return Ok(());
        }
        let project = if project.is_empty() {
            "default"
        } else {
            project
        };
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        let now = now_ns();
        for result in results {
            tx.execute(
                "INSERT INTO assertion_results (project, run_id, assertion_id, pass, detail, evaluated_ns)
                 VALUES (?,?,?,?,?,?)
                 ON CONFLICT(project, run_id, assertion_id) DO UPDATE SET
                   pass = excluded.pass,
                   detail = excluded.detail,
                   evaluated_ns = excluded.evaluated_ns",
                params![
                    project,
                    run_id,
                    result.assertion_id,
                    result.pass,
                    result.detail,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// One run's verdicts with assertion identity, scoped to its project.
    pub fn results_for_run(&self, project: &str, run_id: &str) -> anyhow::Result<Vec<EvalResult>> {
        let project = if project.is_empty() {
            "default"
        } else {
            project
        };
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT r.assertion_id, a.name, a.type, r.pass, r.detail
             FROM assertion_results r JOIN assertions a ON a.id = r.assertion_id
             WHERE r.project = ? AND r.run_id = ? ORDER BY r.assertion_id",
        )?;
        let rows = stmt.query_map(params![project, run_id], |row| {
            let raw_type: String = row.get(2)?;
            let kind = AssertionKind::try_from(raw_type.as_str())
                .map_err(|err| super::sql_enum_error(2, err))?;
            Ok(EvalResult {
                assertion_id: row.get(0)?,
                name: row.get(1)?,
                r#type: kind.as_str().to_string(),
                pass: row.get(3)?,
                detail: row.get(4)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}
