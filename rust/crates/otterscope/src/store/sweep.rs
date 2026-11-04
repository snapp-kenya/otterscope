//! Retention: delete old runs, steps, assertion results and raw batches, then
//! actually reclaim the space their text occupied.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use super::nanos;

impl super::Store {
    /// Delete runs (with their steps) started before `cutoff` and raw batches
    /// received before it. Returns the number of runs removed.
    pub fn sweep(&self, cutoff: DateTime<Utc>) -> anyhow::Result<i64> {
        let ns = nanos(cutoff);
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;

        // steps_fts is a standalone FTS5 table with no foreign key to steps,
        // so deleting steps would leave every message and tool payload
        // searchable and on disk forever. Delete its rows first, while the
        // steps they describe are still there to identify them.
        let fts_removed = tx.execute(
            "
            DELETE FROM steps_fts
             WHERE (project, step_id) IN (
                SELECT CASE WHEN project = '' THEN 'default' ELSE project END, id
                  FROM steps WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?1))",
            params![ns],
        )?;
        // Also drop rows that already lost their step (deleted by an older
        // version of this sweep, or left by a failed ingest).
        let orphaned = tx.execute(
            "
            DELETE FROM steps_fts
             WHERE NOT EXISTS (
                SELECT 1 FROM steps s
                 WHERE s.id = steps_fts.step_id
                   AND CASE WHEN s.project = '' THEN 'default' ELSE s.project END = steps_fts.project)",
            [],
        )?;
        tx.execute(
            "DELETE FROM assertion_results WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?)",
            params![ns],
        )?;
        tx.execute(
            "DELETE FROM steps WHERE run_id IN (SELECT id FROM runs WHERE start_ns < ?)",
            params![ns],
        )?;
        let runs = tx.execute("DELETE FROM runs WHERE start_ns < ?", params![ns])? as i64;
        tx.execute("DELETE FROM raw_batches WHERE received_ns < ?", params![ns])?;
        tx.commit()?;

        if runs > 0 || fts_removed > 0 || orphaned > 0 {
            reclaim(&conn);
        }
        Ok(runs)
    }
}

/// Give back the space a sweep freed. Deleting rows leaves their bytes in the
/// file's free pages and in the WAL until the database is rewritten and the WAL
/// checkpointed. Best effort by design: the deletions are already committed, so
/// a lock must not turn a successful sweep into an error.
fn reclaim(conn: &Connection) {
    if let Err(err) = conn.execute("INSERT INTO steps_fts(steps_fts) VALUES('optimize')", []) {
        tracing::warn!(error = %err, "sweep: fts optimize failed");
    }
    if let Err(err) = conn.execute_batch("VACUUM") {
        tracing::warn!(error = %err, "sweep: vacuum failed");
        return;
    }
    // VACUUM leaves the (now useless) WAL behind in WAL mode; truncate it so
    // the pre-sweep pages aren't kept on disk either.
    if let Err(err) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
        tracing::warn!(error = %err, "sweep: wal checkpoint failed");
    }
}
