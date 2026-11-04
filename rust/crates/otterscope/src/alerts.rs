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
}
