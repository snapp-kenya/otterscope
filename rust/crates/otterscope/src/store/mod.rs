//! SQLite persistence. One connection: the store is a single writer, and
//! callers must drain a cursor before issuing another query on the same path
//! (holding a cursor open while writing self-deadlocks).

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::model::InvalidEnumValue;

mod alerts;
mod assertions;
mod audit;
mod backup;
mod ingest;
mod metrics;
mod migrate;
mod projects;
mod runs;
mod shares;
mod stats;
mod sweep;
mod tokens;
mod views;

pub use migrate::MIGRATIONS;

pub use crate::alerts::Rule;

/// Typed store failures. Callers downcast an [`anyhow::Error`] to tell a
/// uniqueness conflict (409) from other write failures. Reads that may miss
/// return `Option`, and deletes that match no rows return `Ok(false)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Conflict,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str("not found"),
            Error::Conflict => f.write_str("conflict"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
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

impl TryFrom<&str> for Scope {
    type Error = InvalidEnumValue;

    fn try_from(value: &str) -> std::result::Result<Self, InvalidEnumValue> {
        match value {
            "read" => Ok(Scope::Read),
            "admin" => Ok(Scope::Admin),
            other => Err(InvalidEnumValue::new("token scope", other)),
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

#[derive(Clone, Default, Serialize, Deserialize)]
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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CreatedToken {
    pub id: String,
    pub token: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Share {
    pub token: String,
    pub created_at: DateTime<Utc>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("name", &self.name)
            .field("ingest_key", &crate::redacted(&self.ingest_key))
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl std::fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedToken")
            .field("id", &self.id)
            .field("token", &crate::redacted(&self.token))
            .field("name", &self.name)
            .field("scope", &self.scope)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Share")
            .field("token", &crate::redacted(&self.token))
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Handle to the single SQLite connection. All access is serialised through the
/// mutex, matching the single-writer design.
pub struct Store {
    conn: Mutex<Connection>,
    path: String,
}

impl Store {
    /// Open (creating if needed), apply migrations and tighten file permissions.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        backup::secure_open(path)?;
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
            path: path.to_string(),
        };
        // Refuse a foreign/legacy database before switching the file to WAL:
        // the refusal must not rewrite anything, not even the journal header.
        store.migrate()?;
        {
            let conn = store.lock()?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
        }
        backup::restrict_perms(path)?;
        Ok(store)
    }

    pub(crate) fn lock(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex poisoned"))
    }

    #[cfg(test)]
    pub(crate) fn with<R>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<R>,
    ) -> anyhow::Result<R> {
        let conn = self.lock()?;
        f(&conn).map_err(Into::into)
    }

    /// Apply pending migrations, snapshotting an existing database first.
    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.lock()?;
        match migrate::plan(&conn)? {
            migrate::Plan::UpToDate => {}
            migrate::Plan::Fresh => migrate::apply(&conn)?,
            migrate::Plan::Pending { applied, pending } => {
                if migrate::needs_snapshot(applied, pending) {
                    backup::backup_before_migrate(&conn, &self.path)?;
                }
                migrate::apply(&conn)?;
            }
            migrate::Plan::Unsupported(reason) => anyhow::bail!("{reason}"),
        }
        Ok(())
    }

    pub fn close(&self) {}
}

pub(crate) fn is_memory_path(path: &str) -> bool {
    path.is_empty() || path.contains(":memory:")
}

pub(crate) fn now_ns() -> i64 {
    nanos(Utc::now())
}

pub(crate) fn nanos(at: DateTime<Utc>) -> i64 {
    at.timestamp_nanos_opt()
        .unwrap_or_else(|| at.timestamp_micros().saturating_mul(1_000))
}

pub(crate) fn datetime(ns: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(ns)
}

pub(crate) fn random_hex(n: usize) -> anyhow::Result<String> {
    use rand::RngCore;

    let mut buf = vec![0u8; n];
    rand::rng().fill_bytes(&mut buf);
    Ok(hex(&buf))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Wrap a constrained-column parse failure in the `rusqlite` error a row
/// mapper has to return. The store never coerces an unknown enum to a
/// default: a corrupt row is an error, not a silent `Running`/`Generic`.
pub(crate) fn sql_enum_error(index: usize, err: InvalidEnumValue) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
}

/// Map a SQLite write error to [`Error::Conflict`] when it is a uniqueness
/// violation, so the SQLite-specific detection stays in this package.
pub(crate) fn classify_write(err: rusqlite::Error) -> anyhow::Error {
    if err.to_string().contains("UNIQUE constraint failed") {
        anyhow::Error::new(Error::Conflict).context(err.to_string())
    } else {
        anyhow::Error::from(err)
    }
}

/// Redact a secret to a logged prefix.
pub(crate) fn short_token(token: &str) -> String {
    if token.chars().count() > 8 {
        let prefix: String = token.chars().take(8).collect();
        format!("{prefix}…")
    } else {
        token.to_string()
    }
}

#[cfg(test)]
mod tests;
