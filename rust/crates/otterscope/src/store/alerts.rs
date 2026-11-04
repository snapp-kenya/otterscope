//! Alert rule persistence. The alerts package owns validation, evaluation and
//! notification; the store only keeps the definitions and their firing state.

use rusqlite::params;

use crate::alerts::Rule;

use super::now_ns;

const ALERT_COLUMNS: &str = "id, project, name, type, threshold, window_secs, config, webhook_url, \
     enabled, firing, settle_secs, pending_since_ns";

impl super::Store {
    /// Store an already-validated alert rule.
    pub fn create_alert(&self, mut rule: Rule) -> anyhow::Result<Rule> {
        if rule.project.is_empty() {
            rule.project = "default".to_string();
        }
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO alerts (project, name, type, threshold, window_secs, config, webhook_url, enabled, settle_secs, created_ns)
             VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                rule.project,
                rule.name,
                rule.r#type,
                rule.threshold,
                rule.window_secs,
                rule.config,
                rule.webhook_url,
                rule.enabled,
                rule.settle_secs,
                now_ns()
            ],
        )
        .map_err(super::classify_write)?;
        rule.id = conn.last_insert_rowid();
        super::audit::record(&conn, "create", "alert", &rule.name);
        Ok(rule)
    }

    /// Alerts for one project, or every project when `project` is empty.
    pub fn list_alerts(&self, project: &str) -> anyhow::Result<Vec<Rule>> {
        let conn = self.lock()?;
        if project.is_empty() {
            query_alerts(
                &conn,
                &format!("SELECT {ALERT_COLUMNS} FROM alerts ORDER BY id"),
                [],
            )
        } else {
            query_alerts(
                &conn,
                &format!("SELECT {ALERT_COLUMNS} FROM alerts WHERE project = ? ORDER BY id"),
                params![project],
            )
        }
    }

    /// Every enabled alert across projects (watcher).
    pub fn list_enabled_alerts(&self) -> anyhow::Result<Vec<Rule>> {
        let conn = self.lock()?;
        query_alerts(
            &conn,
            &format!("SELECT {ALERT_COLUMNS} FROM alerts WHERE enabled = 1 ORDER BY id"),
            [],
        )
    }

    /// Update an alert's firing state; records `last_fired_ns` when
    /// transitioning into firing and clears the settle timer either way.
    pub fn set_alert_firing(&self, id: i64, firing: bool) -> anyhow::Result<()> {
        let conn = self.lock()?;
        if firing {
            conn.execute(
                "UPDATE alerts SET firing = 1, last_fired_ns = ?, pending_since_ns = 0 WHERE id = ?",
                params![now_ns(), id],
            )?;
        } else {
            conn.execute(
                "UPDATE alerts SET firing = 0, pending_since_ns = 0 WHERE id = ?",
                params![id],
            )?;
        }
        Ok(())
    }

    /// Record when a rule's condition flipped away from the last notified
    /// state; 0 clears it (the condition flapped back before settling).
    pub fn set_alert_pending(&self, id: i64, ns: i64) -> anyhow::Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE alerts SET pending_since_ns = ? WHERE id = ?",
            params![ns, id],
        )?;
        Ok(())
    }

    /// Remove an alert. False means there was nothing to remove.
    pub fn delete_alert(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock()?;
        if conn.execute("DELETE FROM alerts WHERE id = ?", params![id])? == 0 {
            return Ok(false);
        }
        super::audit::record(&conn, "delete", "alert", &id.to_string());
        Ok(true)
    }
}

fn query_alerts(
    conn: &rusqlite::Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> anyhow::Result<Vec<Rule>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(args, |row| {
        Ok(Rule {
            id: row.get(0)?,
            project: row.get(1)?,
            name: row.get(2)?,
            r#type: row.get(3)?,
            threshold: row.get(4)?,
            window_secs: row.get(5)?,
            config: row.get(6)?,
            webhook_url: row.get(7)?,
            enabled: row.get(8)?,
            firing: row.get(9)?,
            settle_secs: row.get(10)?,
            pending_since_ns: row.get(11)?,
        })
    })?;
    let mut alerts = Vec::new();
    for row in rows {
        alerts.push(row?);
    }
    Ok(alerts)
}
