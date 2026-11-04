//! Point-in-time instance counts for `/metrics`.

use std::fs;

use super::{Metrics, is_memory_path};

impl super::Store {
    /// Gather instance counts on demand, so Prometheus can derive rates from
    /// successive scrapes.
    pub fn metrics(&self) -> anyhow::Result<Metrics> {
        let conn = self.lock()?;
        let mut metrics = Metrics::default();
        conn.query_row(
            "
            SELECT count(*),
                   coalesce(sum(status='ok'),0),
                   coalesce(sum(status='error'),0),
                   coalesce(sum(status='running'),0)
            FROM runs",
            [],
            |row| {
                metrics.runs = row.get(0)?;
                metrics.runs_ok = row.get(1)?;
                metrics.runs_error = row.get(2)?;
                metrics.runs_running = row.get(3)?;
                Ok(())
            },
        )?;
        metrics.steps = conn.query_row("SELECT count(*) FROM steps", [], |row| row.get(0))?;
        metrics.projects = conn.query_row("SELECT count(*) FROM projects", [], |row| row.get(0))?;
        metrics.alerts_firing =
            conn.query_row("SELECT coalesce(sum(firing),0) FROM alerts", [], |row| {
                row.get(0)
            })?;
        metrics.db_size_bytes = db_size_bytes(&self.path);
        Ok(metrics)
    }
}

/// The store's real on-disk footprint: the main database file plus its WAL and
/// SHM siblings. In WAL mode most recent data lives in the WAL, so reporting
/// the main file alone understates the size by orders of magnitude.
pub(crate) fn db_size_bytes(path: &str) -> i64 {
    if is_memory_path(path) {
        return 0;
    }
    let mut total = 0i64;
    for p in [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-shm"),
    ] {
        if let Ok(meta) = fs::metadata(p) {
            total += meta.len() as i64;
        }
    }
    total
}
