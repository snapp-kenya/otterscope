//! Retention: delete old runs and raw batches, then actually reclaim the space
//! their text occupied.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use super::nanos;

impl super::Store {
    /// Delete runs started before `cutoff` and raw batches received before it.
    /// Steps, assertion results and shares go with their run through foreign
    /// key cascades, and the FTS triggers purge their index rows. Returns the
    /// number of runs removed.
    pub fn sweep(&self, cutoff: DateTime<Utc>) -> anyhow::Result<i64> {
        let ns = nanos(cutoff);
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        let runs = tx.execute("DELETE FROM runs WHERE start_ns < ?", params![ns])? as i64;
        tx.execute("DELETE FROM raw_batches WHERE received_ns < ?", params![ns])?;
        tx.commit()?;

        if runs > 0 {
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
    if let Err(err) = conn.execute_batch("VACUUM") {
        tracing::warn!(error = %err, "sweep: vacuum failed");
        return;
    }
    // `steps` has no INTEGER PRIMARY KEY, and VACUUM is allowed to renumber
    // rowids of such tables. The external-content FTS index is keyed by that
    // rowid, so rebuild it from the content table after every VACUUM rather
    // than trusting the old rowids to survive.
    if let Err(err) = conn.execute("INSERT INTO steps_fts(steps_fts) VALUES('rebuild')", []) {
        tracing::warn!(error = %err, "sweep: fts rebuild failed");
    } else if let Err(err) = conn.execute("INSERT INTO steps_fts(steps_fts) VALUES('optimize')", [])
    {
        tracing::warn!(error = %err, "sweep: fts optimize failed");
    }
    // VACUUM leaves the (now useless) WAL behind in WAL mode; truncate it so
    // the pre-sweep pages aren't kept on disk either.
    if let Err(err) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
        tracing::warn!(error = %err, "sweep: wal checkpoint failed");
    }
}
