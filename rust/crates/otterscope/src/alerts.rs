//! Alert rules and the settle-then-notify watcher (port of `internal/alerts`).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub project: String,
    pub name: String,
    pub r#type: String,
    pub threshold: f64,
    pub window_secs: i64,
    pub config: String,
    pub webhook_url: String,
    pub enabled: bool,
    pub firing: bool,
    pub settle_secs: i64,
    /// When the condition first flipped away from `firing`; 0 means nothing is
    /// pending. Read-only to API clients; the watcher owns it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_since_ns: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}
