//! The boundary between the HTTP API and the rest of the process.
//!
//! Every call the server makes into `store`, `evals`, `alerts` or `ingest`
//! goes through [`ApiBackend`]. The production implementation lives in
//! `backend_real`; tests substitute an in-memory backend, which is what keeps
//! the guard, auth and wire-shape assertions runnable without the real store.

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::evals::Assertion;
use crate::model::{Run, Step};
use crate::store::{Filter, Metrics, Scope};

#[derive(Debug)]
pub enum BackendError {
    NotFound,
    Conflict,
    Other(anyhow::Error),
}

impl BackendError {
    pub fn other(err: impl Into<anyhow::Error>) -> Self {
        BackendError::Other(err.into())
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotFound => f.write_str("not found"),
            BackendError::Conflict => f.write_str("conflict"),
            BackendError::Other(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<anyhow::Error> for BackendError {
    fn from(err: anyhow::Error) -> Self {
        BackendError::Other(err)
    }
}

pub type BackendResult<T> = Result<T, BackendError>;

/// Translate a store failure into the HTTP-relevant class. The chain is
/// walked because the store wraps a typed error in a context message.
pub(crate) fn map_store_error(err: anyhow::Error) -> BackendError {
    let mut not_found = false;
    let mut conflict = false;
    for cause in err.chain() {
        if let Some(error) = cause.downcast_ref::<crate::store::Error>() {
            match error {
                crate::store::Error::NotFound => not_found = true,
                crate::store::Error::Conflict => conflict = true,
            }
        }
    }
    if conflict {
        BackendError::Conflict
    } else if not_found {
        BackendError::NotFound
    } else {
        BackendError::Other(err)
    }
}

/// A token's identity — never its secret.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// The one response that carries a usable secret.
#[derive(Debug, Clone)]
pub struct CreatedTokenRecord {
    pub id: String,
    pub token: String,
    pub name: String,
    pub scope: Scope,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ShareRecord {
    pub token: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ViewRecord {
    pub id: i64,
    pub name: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub at: DateTime<Utc>,
    pub action: String,
    pub entity: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct AlertRecord {
    pub id: i64,
    pub project: String,
    pub name: String,
    pub kind: String,
    pub threshold: f64,
    pub window_secs: i64,
    pub config: String,
    pub webhook_url: String,
    pub enabled: bool,
    pub firing: bool,
    pub settle_secs: i64,
    pub pending_since_ns: i64,
}

/// A create-alert request, already past JSON decoding. Watcher-owned state is
/// not part of it: a client can never set `firing` or `pendingSinceNs`.
#[derive(Debug, Clone, Default)]
pub struct NewAlert {
    pub project: String,
    pub name: String,
    pub kind: String,
    pub threshold: f64,
    pub window_secs: i64,
    pub config: String,
    pub webhook_url: String,
    pub settle_secs: i64,
}

#[derive(Debug, Clone, Default)]
pub struct AssertionRateRecord {
    pub assertion_id: i64,
    pub name: String,
    pub passed: i64,
    pub total: i64,
}

/// The `/api/stats` payload, projected here so the wire shape does not depend
/// on the store's internal struct.
#[derive(Debug, Clone, Default)]
pub struct StatsRecord {
    pub runs: i64,
    pub errors: i64,
    pub avg_duration_ms: f64,
    pub p50_duration_ms: i64,
    pub p95_duration_ms: i64,
    pub total_cost_usd: Option<f64>,
    pub avg_cost_usd: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub assertion_rates: Vec<AssertionRateRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct EvaluateOptions {
    pub project: String,
    pub run_id: String,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// 0 = the caller did not set one.
    pub limit: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateReport {
    pub runs_evaluated: i64,
    pub runs_skipped: i64,
    pub run_limit: i64,
    pub judge_calls: i64,
    pub judge_calls_skipped: i64,
    pub judge_call_limit: i64,
}

/// Everything the HTTP layer needs from the rest of the process.
pub trait ApiBackend: Send + Sync {
    fn list_runs(&self, filter: &Filter, limit: i64, offset: i64) -> BackendResult<Vec<Run>>;
    fn get_run(&self, id: &str) -> BackendResult<(Run, Vec<Step>)>;
    fn get_run_in_project(&self, project: &str, id: &str) -> BackendResult<(Run, Vec<Step>)>;
    fn get_stats(&self, filter: &Filter) -> BackendResult<StatsRecord>;
    fn metrics(&self) -> BackendResult<Metrics>;

    fn list_assertions(&self, project: &str) -> BackendResult<Vec<Assertion>>;
    fn create_assertion(&self, assertion: Assertion) -> BackendResult<Assertion>;
    fn delete_assertion(&self, id: i64) -> BackendResult<()>;
    fn results_for_run(
        &self,
        project: &str,
        run_id: &str,
    ) -> BackendResult<Vec<crate::evals::Result>>;

    /// On-demand evaluation. Async because the judge path speaks HTTP.
    fn evaluate<'a>(
        &'a self,
        options: &'a EvaluateOptions,
    ) -> Pin<Box<dyn Future<Output = BackendResult<EvaluateReport>> + Send + 'a>>;

    fn list_alerts(&self, project: &str) -> BackendResult<Vec<AlertRecord>>;
    fn create_alert(&self, alert: &NewAlert) -> BackendResult<AlertRecord>;
    fn delete_alert(&self, id: i64) -> BackendResult<()>;

    fn create_share(&self, project: &str, run_id: &str) -> BackendResult<String>;
    fn resolve_share(&self, token: &str) -> Option<(String, String)>;
    fn shares_for_run(&self, project: &str, run_id: &str) -> BackendResult<Vec<ShareRecord>>;
    fn delete_share(&self, token: &str) -> BackendResult<()>;

    fn list_views(&self) -> BackendResult<Vec<ViewRecord>>;
    fn create_view(&self, name: &str, params: &serde_json::Value) -> BackendResult<ViewRecord>;
    fn delete_view(&self, id: i64) -> BackendResult<()>;

    /// Audit is best-effort in the store, which returns the entries directly.
    fn list_audit(&self, limit: i64) -> Vec<AuditRecord>;

    fn list_tokens(&self) -> BackendResult<Vec<TokenRecord>>;
    fn create_token(&self, name: &str, scope: Scope) -> BackendResult<CreatedTokenRecord>;
    fn delete_token(&self, id: &str) -> BackendResult<()>;
    fn authenticate_token(&self, token: &str) -> Option<TokenRecord>;

    fn validate_assertion(&self, assertion: &Assertion) -> Result<(), String>;
    fn assertion_judge_model(&self, assertion: &Assertion) -> Result<String, String>;
    fn check_judge_model(&self, model: &str) -> Result<(), String>;
    /// Async because alert validation resolves the webhook host (SSRF gate).
    fn validate_alert<'a>(
        &'a self,
        alert: &'a NewAlert,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    fn redact_webhook_url(&self, raw: &str) -> String;

    /// The MCP router, when the module is present. Mounted inside the same
    /// guard/auth stack as the API.
    fn mcp_router(&self) -> Option<axum::Router> {
        None
    }
}
