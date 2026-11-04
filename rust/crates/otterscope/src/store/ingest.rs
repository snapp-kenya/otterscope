//! Raw-ingest persistence: the original gzip-compressed OTLP payload and its
//! normalized steps, committed together.

use rusqlite::params;

use crate::model::Step;

use super::{now_ns, runs::upsert_steps_tx};

impl super::Store {
    /// Persist a raw ingested batch and its normalized steps in one
    /// transaction. Returns the number of steps refused because an existing
    /// (project, span ID) came from a different resource service: a batch that
    /// was entirely rejected keeps no replayable copy.
    pub fn ingest_batch(&self, project: &str, raw: &[u8], steps: &[Step]) -> anyhow::Result<usize> {
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        // A batch is project-scoped even when it normalizes to no steps.
        super::projects::ensure_project(&tx, project)?;
        let rejected = upsert_steps_tx(&tx, steps)?;
        if !steps.is_empty() && rejected == steps.len() {
            // Nothing was written; keep no replayable copy of a refused batch.
            tx.commit()?;
            return Ok(rejected);
        }
        tx.execute(
            "INSERT INTO raw_batches (received_ns, payload, project) VALUES (?, ?, ?)",
            params![now_ns(), raw, project],
        )?;
        tx.commit()?;
        Ok(rejected)
    }

    /// Stream every stored raw payload to `f` in insertion order, stopping on
    /// the first error. Each page's cursor is drained and closed before `f`
    /// runs, so a write inside `f` cannot self-deadlock the single connection.
    pub fn each_raw_batch(
        &self,
        mut f: impl FnMut(&str, &[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        const PAGE_SIZE: i64 = 100;
        let mut after_id = 0i64;
        loop {
            let (batches, max_id) = {
                let conn = self.lock()?;
                let mut stmt = conn.prepare(
                    "SELECT id, project, payload FROM raw_batches WHERE id > ? ORDER BY id LIMIT ?",
                )?;
                let mut rows = stmt.query(params![after_id, PAGE_SIZE])?;
                let mut batches = Vec::new();
                let mut max_id = 0i64;
                while let Some(row) = rows.next()? {
                    let id: i64 = row.get(0)?;
                    let project: String = row.get(1)?;
                    let payload: Vec<u8> = row.get(2)?;
                    max_id = id;
                    batches.push((project, payload));
                }
                (batches, max_id)
            };
            if batches.is_empty() {
                return Ok(());
            }
            for (project, payload) in &batches {
                f(project, payload)?;
            }
            after_id = max_id;
        }
    }
}
