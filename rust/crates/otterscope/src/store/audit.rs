//! Change history: what mutated, and when.

use rusqlite::{Connection, params};

use super::{AuditEntry, datetime, now_ns};

/// Record a mutation. Best effort: an audit failure must never fail the
/// operation it describes.
pub(crate) fn record(conn: &Connection, action: &str, entity: &str, detail: &str) {
    if let Err(err) = conn.execute(
        "INSERT INTO audit_log (at_ns, action, entity, detail) VALUES (?,?,?,?)",
        params![now_ns(), action, entity, detail],
    ) {
        tracing::warn!(error = %err, "could not record audit entry");
    }
}

impl super::Store {
    /// Recent audit entries, newest first. Returns an empty list if the log
    /// cannot be read (audit history must not break a caller).
    pub fn list_audit(&self, limit: i64) -> Vec<AuditEntry> {
        let limit = if limit <= 0 || limit > 1000 {
            200
        } else {
            limit
        };
        let Ok(conn) = self.lock() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(
            "SELECT at_ns, action, entity, detail FROM audit_log ORDER BY at_ns DESC LIMIT ?",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                tracing::warn!(error = %err, "could not read audit log");
                return Vec::new();
            }
        };
        let rows = stmt.query_map(params![limit], |row| {
            let at_ns: i64 = row.get(0)?;
            Ok(AuditEntry {
                at: datetime(at_ns),
                action: row.get(1)?,
                entity: row.get(2)?,
                detail: row.get(3)?,
            })
        });
        let Ok(rows) = rows else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    tracing::warn!(error = %err, "could not read audit entry");
                    break;
                }
            }
        }
        entries
    }
}
