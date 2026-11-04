//! Opt-in public read-only links to a single run.

use rusqlite::{OptionalExtension, params};

use super::{Share, datetime, now_ns, random_hex};

impl super::Store {
    /// Mint an unguessable token for (project, run_id).
    pub fn create_share(&self, project: &str, run_id: &str) -> anyhow::Result<String> {
        let token = random_hex(16)?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO shares (token, project, run_id, created_ns) VALUES (?,?,?,?)",
            params![token, project, run_id, now_ns()],
        )?;
        super::audit::record(&conn, "create", "share", run_id);
        Ok(token)
    }

    /// Map a token to its (project, run_id); `None` if unknown.
    pub fn resolve_share(&self, token: &str) -> anyhow::Result<Option<(String, String)>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT project, run_id FROM shares WHERE token = ?",
                params![token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// Active shares for a run.
    pub fn shares_for_run(&self, project: &str, run_id: &str) -> anyhow::Result<Vec<Share>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT token, created_ns FROM shares WHERE project = ? AND run_id = ? ORDER BY created_ns",
        )?;
        let rows = stmt.query_map(params![project, run_id], |row| {
            let created_ns: i64 = row.get(1)?;
            Ok(Share {
                token: row.get(0)?,
                created_at: datetime(created_ns),
            })
        })?;
        let mut shares = Vec::new();
        for row in rows {
            shares.push(row?);
        }
        Ok(shares)
    }

    /// Revoke a share token. False means there was nothing to revoke, so
    /// nothing is audited.
    ///
    /// The audit entry records the token's SHA-256 digest, never a prefix or
    /// any other part of the token: the digest is irreversible, and an
    /// operator who still holds the token can recompute it to correlate the
    /// revocation with the link it revoked.
    pub fn delete_share(&self, token: &str) -> anyhow::Result<bool> {
        let conn = self.lock()?;
        if conn.execute("DELETE FROM shares WHERE token = ?", params![token])? == 0 {
            return Ok(false);
        }
        super::audit::record(&conn, "delete", "share", &super::tokens::token_hash(token));
        Ok(true)
    }
}
