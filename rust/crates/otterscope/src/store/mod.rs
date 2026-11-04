//! SQLite persistence. One connection, as in the Go version: the store is a
//! single writer, and callers must drain a cursor before issuing another query
//! on the same path (holding a cursor open while writing self-deadlocks).

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

mod migrate;

pub use migrate::MIGRATIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Admin,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filter {
    pub project: String,
    pub status: String,
    pub service: String,
    pub model: String,
    pub prompt: String,
    pub query: String,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub runs: i64,
    pub errors: i64,
    pub avg_duration_ms: f64,
    pub p50_duration_ms: i64,
    pub p95_duration_ms: i64,
    pub total_cost_usd: Option<f64>,
    pub avg_cost_usd: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub assertion_rates: Vec<AssertionRate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssertionRate {
    pub name: String,
    pub pass: i64,
    pub fail: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub runs: i64,
    pub runs_ok: i64,
    pub runs_error: i64,
    pub runs_running: i64,
    pub steps: i64,
    pub projects: i64,
    pub alerts_firing: i64,
    pub db_size_bytes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub ingest_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Token {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreatedToken {
    pub id: String,
    pub token: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
}

impl Default for Scope {
    fn default() -> Self {
        Scope::Read
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedView {
    pub id: i64,
    pub name: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    pub action: String,
    pub entity: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Share {
    pub token: String,
    pub created_at: DateTime<Utc>,
}

/// Handle to the single SQLite connection. All access is serialised through the
/// mutex, matching the Go store's one-writer design.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (creating if needed), apply migrations and tighten file permissions.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> anyhow::Result<R> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("store mutex poisoned"))?;
        f(&conn).map_err(Into::into)
    }

    pub(crate) fn migrate(&self) -> anyhow::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex poisoned"))?;
        migrate::apply(&conn)
    }

    pub fn close(&self) {}
}

#[cfg(test)]
mod tests;
