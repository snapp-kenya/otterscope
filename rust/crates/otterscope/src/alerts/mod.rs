//! Alert rules and the settle-then-notify watcher (ported from the original Go
//! implementation).
//!
//! A rule is a threshold over the store's run aggregates, so an alert is just a
//! filtered window: the watcher reads [`crate::store::Stats`] and decides.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::InvalidEnumValue;
use crate::store::Stats;

mod watcher;
mod webhook;

pub use watcher::{AlertStore, Clock, Notification, Watcher};
pub use webhook::{
    Lookup, MAX_WEBHOOK_REDIRECTS, WebhookGate, check_webhook_addr, post_webhook,
    redact_webhook_url, validate_webhook_url, validate_webhook_url_with, webhook_gate,
};

/// Bounds the flap-suppression window (24h).
pub const MAX_SETTLE_SECS: i64 = 24 * 60 * 60;

/// The alert metrics the watcher can evaluate. The database CHECK constraint
/// and this enum must agree; the store decodes rows through [`AlertKind`] so
/// an invalid value cannot round-trip into a [`Rule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    ErrorRate,
    Cost,
    P95Latency,
    AssertionFailRate,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::ErrorRate => "error_rate",
            AlertKind::Cost => "cost",
            AlertKind::P95Latency => "p95_latency",
            AlertKind::AssertionFailRate => "assertion_fail_rate",
        }
    }
}

impl TryFrom<&str> for AlertKind {
    type Error = InvalidEnumValue;

    fn try_from(value: &str) -> std::result::Result<Self, InvalidEnumValue> {
        match value {
            "error_rate" => Ok(AlertKind::ErrorRate),
            "cost" => Ok(AlertKind::Cost),
            "p95_latency" => Ok(AlertKind::P95Latency),
            "assertion_fail_rate" => Ok(AlertKind::AssertionFailRate),
            other => Err(InvalidEnumValue::new("alert type", other)),
        }
    }
}

impl std::fmt::Debug for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rule")
            .field("id", &self.id)
            .field("project", &self.project)
            .field("name", &self.name)
            .field("type", &self.r#type)
            .field("threshold", &self.threshold)
            .field("window_secs", &self.window_secs)
            .field("config", &self.config)
            .field(
                "webhook_url",
                &webhook::redact_webhook_url(&self.webhook_url),
            )
            .field("enabled", &self.enabled)
            .field("firing", &self.firing)
            .field("settle_secs", &self.settle_secs)
            .field("pending_since_ns", &self.pending_since_ns)
            .finish()
    }
}

impl Rule {
    /// Decode the stored/wire spelling into the typed kind.
    pub fn kind(&self) -> std::result::Result<AlertKind, InvalidEnumValue> {
        AlertKind::try_from(self.r#type.as_str())
    }
}

/// One alert definition. The store persists it; this module owns the behaviour
/// (validate, evaluate, notify).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: i64,
    pub project: String,
    pub name: String,
    /// `error_rate` | `cost` | `p95_latency` | `assertion_fail_rate`
    pub r#type: String,
    pub threshold: f64,
    pub window_secs: i64,
    /// Assertion name for `assertion_fail_rate`.
    pub config: String,
    pub webhook_url: String,
    pub enabled: bool,
    pub firing: bool,
    /// How long the condition must hold its new state before the watcher
    /// notifies, in both directions. 0 notifies immediately.
    pub settle_secs: i64,
    /// When the condition first flipped away from [`Rule::firing`], or 0 when
    /// nothing is pending. Read-only to API clients; the watcher owns it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_since_ns: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Checks a rule's type/config/threshold before storing it. The webhook URL is
/// resolved here, so an internal address is refused at creation time as well as
/// before every POST.
pub async fn validate(rule: &Rule) -> Result<()> {
    if rule.name.is_empty() {
        bail!("alert needs a name");
    }
    if rule.webhook_url.is_empty() {
        bail!("alert needs a webhook URL");
    }
    validate_webhook_url(&rule.webhook_url).await?;
    if rule.window_secs <= 0 {
        bail!("windowSecs must be positive");
    }
    if rule.settle_secs < 0 {
        bail!("settleSecs cannot be negative");
    }
    if rule.settle_secs > MAX_SETTLE_SECS {
        bail!(
            "settleSecs {} is over the {} second maximum (is that milliseconds?)",
            rule.settle_secs,
            MAX_SETTLE_SECS
        );
    }
    let kind = match rule.kind() {
        Ok(kind) => kind,
        Err(_) => bail!("unknown alert type {:?}", rule.r#type),
    };
    match kind {
        AlertKind::ErrorRate | AlertKind::AssertionFailRate => {
            if !(0.0..=1.0).contains(&rule.threshold) {
                bail!("{} threshold is a fraction 0..1", rule.r#type);
            }
            if kind == AlertKind::AssertionFailRate && rule.config.is_empty() {
                bail!("assertion_fail_rate needs config = assertion name");
            }
        }
        AlertKind::Cost | AlertKind::P95Latency => {
            if rule.threshold <= 0.0 {
                bail!("{} threshold must be positive", rule.r#type);
            }
        }
    }
    Ok(())
}

/// One rule's reading. `firable == false` means the metric is not computable
/// (no runs, unknown assertion) and the rule's state must be left as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub firing: bool,
    pub firable: bool,
    pub detail: String,
}

impl Evaluation {
    fn unknown() -> Self {
        Self {
            firing: false,
            firable: false,
            detail: String::new(),
        }
    }
}

/// Computes whether the rule is currently firing over `stats`, plus a
/// human-readable detail.
pub fn evaluate(rule: &Rule, stats: &Stats) -> Evaluation {
    let Ok(kind) = rule.kind() else {
        return Evaluation::unknown();
    };
    match kind {
        AlertKind::ErrorRate => {
            if stats.runs == 0 {
                return Evaluation::unknown();
            }
            let rate = stats.errors as f64 / stats.runs as f64;
            Evaluation {
                firing: rate > rule.threshold,
                firable: true,
                detail: format!(
                    "error rate {:.1}% over {} runs (threshold {:.1}%)",
                    rate * 100.0,
                    stats.runs,
                    rule.threshold * 100.0
                ),
            }
        }
        AlertKind::Cost => {
            let Some(cost) = stats.total_cost_usd else {
                return Evaluation::unknown();
            };
            Evaluation {
                firing: cost > rule.threshold,
                firable: true,
                detail: format!(
                    "window cost ${:.4} (threshold ${:.4})",
                    cost, rule.threshold
                ),
            }
        }
        AlertKind::P95Latency => {
            if stats.runs == 0 {
                return Evaluation::unknown();
            }
            Evaluation {
                firing: stats.p95_duration_ms as f64 > rule.threshold,
                firable: true,
                detail: format!(
                    "p95 latency {}ms (threshold {:.0}ms)",
                    stats.p95_duration_ms, rule.threshold
                ),
            }
        }
        AlertKind::AssertionFailRate => {
            let Some(rate) = stats.assertion_rates.iter().find(|a| a.name == rule.config) else {
                return Evaluation::unknown();
            };
            let total = rate.pass + rate.fail;
            if total == 0 {
                return Evaluation::unknown();
            }
            let fail_rate = rate.fail as f64 / total as f64;
            Evaluation {
                firing: fail_rate > rule.threshold,
                firable: true,
                detail: format!(
                    "{:?} fail rate {:.1}% over {} runs (threshold {:.1}%)",
                    rate.name,
                    fail_rate * 100.0,
                    total,
                    rule.threshold * 100.0
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::AssertionRate;

    const HOOK: &str = "https://93.184.216.34/hook";

    fn rule(r#type: &str, threshold: f64) -> Rule {
        Rule {
            name: "n".into(),
            webhook_url: HOOK.into(),
            window_secs: 3600,
            r#type: r#type.into(),
            threshold,
            ..Rule::default()
        }
    }

    #[tokio::test]
    async fn validate_accepts_every_type() {
        let mut assertion = rule("assertion_fail_rate", 0.2);
        assertion.config = "a".into();
        for good in [
            rule("error_rate", 0.1),
            rule("cost", 5.0),
            rule("p95_latency", 1000.0),
            assertion,
        ] {
            assert!(validate(&good).await.is_ok(), "{} rejected", good.r#type);
        }
    }

    #[tokio::test]
    async fn validate_rejects_bad_rules() {
        let mut nameless = rule("cost", 1.0);
        nameless.name.clear();
        let mut hookless = rule("cost", 1.0);
        hookless.webhook_url.clear();
        let mut missing_config = rule("assertion_fail_rate", 0.2);
        missing_config.config.clear();
        for bad in [
            nameless,
            hookless,
            rule("error_rate", 5.0),
            missing_config,
            rule("bogus", 1.0),
        ] {
            assert!(
                validate(&bad).await.is_err(),
                "{} should be invalid",
                bad.r#type
            );
        }
    }

    #[tokio::test]
    async fn validate_uses_the_webhook_gate() {
        let mut internal = rule("cost", 1.0);
        internal.webhook_url = "http://127.0.0.1:9000/hook".into();
        assert!(validate(&internal).await.is_err());
        let mut relative = rule("cost", 1.0);
        relative.webhook_url = "hooks.slack.com/services/T/B/x".into();
        assert!(validate(&relative).await.is_err());
    }

    #[test]
    fn evaluate_reads_each_metric() {
        let stats = Stats {
            runs: 10,
            errors: 3,
            p95_duration_ms: 8000,
            total_cost_usd: Some(2.5),
            assertion_rates: vec![AssertionRate {
                name: "polite".into(),
                pass: 6,
                fail: 4,
            }],
            ..Stats::default()
        };
        let cases = [
            (rule("error_rate", 0.2), true, true),
            (rule("error_rate", 0.5), false, true),
            (rule("cost", 2.0), true, true),
            (rule("p95_latency", 5000.0), true, true),
            (rule("p95_latency", 9000.0), false, true),
            (
                Rule {
                    config: "polite".into(),
                    ..rule("assertion_fail_rate", 0.3)
                },
                true,
                true,
            ),
            (
                Rule {
                    config: "polite".into(),
                    ..rule("assertion_fail_rate", 0.5)
                },
                false,
                true,
            ),
            (
                Rule {
                    config: "missing".into(),
                    ..rule("assertion_fail_rate", 0.1)
                },
                false,
                false,
            ),
        ];
        for (r, firing, firable) in cases {
            let got = evaluate(&r, &stats);
            assert_eq!(
                (got.firing, got.firable),
                (firing, firable),
                "{}/{}",
                r.r#type,
                r.config
            );
        }
    }

    #[test]
    fn evaluate_is_not_firable_without_data() {
        let empty = Stats::default();
        for r in [rule("error_rate", 0.1), rule("p95_latency", 1.0)] {
            assert!(!evaluate(&r, &empty).firable);
        }
        let mut no_cost = empty;
        no_cost.runs = 5;
        assert!(!evaluate(&rule("cost", 1.0), &no_cost).firable);
    }

    #[tokio::test]
    async fn validate_settle_bounds() {
        for secs in [0, 1, 300, MAX_SETTLE_SECS] {
            let mut r = rule("error_rate", 0.5);
            r.settle_secs = secs;
            assert!(validate(&r).await.is_ok(), "settleSecs={secs} rejected");
        }
        for secs in [-1, MAX_SETTLE_SECS + 1, 300_000] {
            let mut r = rule("error_rate", 0.5);
            r.settle_secs = secs;
            assert!(validate(&r).await.is_err(), "settleSecs={secs} accepted");
        }
    }

    #[test]
    fn rule_json_uses_the_go_field_names() {
        let mut r = rule("error_rate", 0.5);
        r.pending_since_ns = 0;
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(json["type"], "error_rate");
        assert_eq!(json["webhookUrl"], HOOK);
        assert_eq!(json["windowSecs"], 3600);
        assert_eq!(json["settleSecs"], 0);
        assert!(json.get("pendingSinceNs").is_none());
        r.pending_since_ns = 42;
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(json["pendingSinceNs"], 42);
    }
}
