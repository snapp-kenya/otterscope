//! The periodic watcher: evaluates enabled rules, applies the settle window
//! and POSTs webhooks on state transitions.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use super::webhook::{WebhookGate, new_webhook_client, post_webhook, webhook_gate};
use super::{Rule, evaluate};
use crate::store::{Filter, Stats};

/// The subset of the store the watcher needs. Kept small so tests can drive
/// the watcher without a database.
pub trait AlertStore: Send + Sync {
    fn list_enabled_alerts(&self) -> impl Future<Output = Result<Vec<Rule>>> + Send;
    fn get_stats(&self, filter: &Filter) -> impl Future<Output = Result<Stats>> + Send;
    fn set_alert_firing(&self, id: i64, firing: bool) -> impl Future<Output = Result<()>> + Send;
    fn set_alert_pending(&self, id: i64, ns: i64) -> impl Future<Output = Result<()>> + Send;
}

/// The wall clock, injected so settle tests can script time.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The JSON payload POSTed to an alert's webhook. It carries alert data only —
/// never secrets or environment (see issue #48).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub alert: String,
    pub project: String,
    pub r#type: String,
    /// "firing" | "resolved"
    pub status: String,
    pub detail: String,
    pub threshold: f64,
    pub fired_at: String,
}

impl Notification {
    /// The one-line human summary used in Slack/Discord bodies.
    pub fn message(&self) -> String {
        let (icon, label) = if self.status == "firing" {
            ("\u{1f534}", "firing")
        } else {
            ("\u{2705}", "resolved")
        };
        format!(
            "{icon} [{label}] {} ({}): {}",
            self.alert, self.project, self.detail
        )
    }
}

/// Periodically evaluates enabled alerts and fires webhooks on
/// ok→firing / firing→ok transitions. Owned by the server lifecycle.
pub struct Watcher<W: AlertStore> {
    config: Config<W>,
    handle: Option<JoinHandle<()>>,
}

impl<W: AlertStore + 'static> Watcher<W> {
    /// Creates a watcher evaluating every `interval`.
    pub fn new(store: Arc<W>, interval: Duration) -> Result<Self> {
        Ok(Self {
            config: Config {
                store,
                interval,
                client: new_webhook_client()?,
                gate: webhook_gate(),
                clock: Arc::new(Utc::now),
            },
            handle: None,
        })
    }

    /// Replaces the SSRF gate (tests whose webhook is a loopback server).
    pub fn with_webhook_gate(mut self, gate: WebhookGate) -> Self {
        self.config.gate = gate;
        self
    }

    /// Replaces the clock (settle tests).
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.config.clock = clock;
        self
    }

    /// Launches the evaluation loop.
    pub fn start(&mut self) {
        let config = Arc::new(self.config.clone());
        self.handle = Some(tokio::spawn(async move { config.run().await }));
    }

    /// Cancels any in-flight evaluation (including its webhook POSTs) and
    /// waits for the loop to exit.
    pub async fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Runs a single evaluation cycle synchronously (tests and a possible
    /// future manual trigger).
    pub async fn evaluate_once(&self) {
        self.config.evaluate_once().await;
    }
}

impl<W: AlertStore> Drop for Watcher<W> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

struct Config<W: AlertStore> {
    store: Arc<W>,
    interval: Duration,
    client: reqwest::Client,
    gate: WebhookGate,
    clock: Clock,
}

impl<W: AlertStore> Clone for Config<W> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            interval: self.interval,
            client: self.client.clone(),
            gate: self.gate.clone(),
            clock: self.clock.clone(),
        }
    }
}

impl<W: AlertStore + 'static> Config<W> {
    async fn run(self: Arc<Self>) {
        let interval = self.interval.max(Duration::from_millis(1));
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick resolves immediately; discard it, like a Go Ticker.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if tokio::time::timeout(interval, self.evaluate_once())
                .await
                .is_err()
            {
                tracing::warn!("alerts: evaluation cycle timed out");
            }
        }
    }

    async fn evaluate_once(&self) {
        let rules = match self.store.list_enabled_alerts().await {
            Ok(rules) => rules,
            Err(err) => {
                tracing::error!(%err, "alerts: list failed");
                return;
            }
        };
        // Cache stats per (project, window) — many alerts share a window.
        let mut stats_cache: HashMap<(String, i64), Stats> = HashMap::new();

        for rule in rules {
            let key = (rule.project.clone(), rule.window_secs);
            let stats = match stats_cache.get(&key) {
                Some(stats) => stats.clone(),
                None => {
                    let since = self.now() - chrono::Duration::seconds(rule.window_secs);
                    let filter = Filter {
                        project: rule.project.clone(),
                        since: Some(since),
                        ..Filter::default()
                    };
                    match self.store.get_stats(&filter).await {
                        Ok(stats) => {
                            stats_cache.insert(key, stats.clone());
                            stats
                        }
                        Err(err) => {
                            tracing::error!(alert = %rule.name, %err, "alerts: stats failed");
                            continue;
                        }
                    }
                }
            };

            let reading = evaluate(&rule, &stats);
            if !reading.firable {
                continue;
            }
            if !self.ready_to_notify(&rule, reading.firing).await {
                continue;
            }
            if reading.firing && !rule.firing {
                self.notify(&rule, "firing", &reading.detail).await;
                if let Err(err) = self.store.set_alert_firing(rule.id, true).await {
                    tracing::error!(alert = %rule.name, %err, "alerts: set firing failed");
                }
            } else if !reading.firing && rule.firing {
                self.notify(&rule, "resolved", &reading.detail).await;
                if let Err(err) = self.store.set_alert_firing(rule.id, false).await {
                    tracing::error!(alert = %rule.name, %err, "alerts: clear firing failed");
                }
            }
        }
    }

    /// Whether the rule should send a notification for its current reading.
    /// `false` covers two situations: the reading matches the state we last
    /// notified about, or the condition flipped but has not held long enough.
    ///
    /// Flap suppression is symmetric on purpose: the condition must hold its
    /// new state for [`Rule::settle_secs`] before we notify in either
    /// direction. Suppressing only the firing edge still lets a value hovering
    /// at the threshold produce a firing/resolved pair every settle window;
    /// delaying the resolve too means a genuinely oscillating metric produces
    /// nothing at all, which is the point. The cost is that a real recovery is
    /// reported up to a settle window late, and that is the safe direction to
    /// be wrong in.
    async fn ready_to_notify(&self, rule: &Rule, firing: bool) -> bool {
        if firing == rule.firing {
            // Steady state. If a transition was pending it just flapped back,
            // so the timer restarts from scratch next time.
            if rule.pending_since_ns != 0 {
                self.clear_pending(rule).await;
            }
            return false;
        }
        if rule.settle_secs <= 0 {
            return true;
        }
        let now = self.now();
        if rule.pending_since_ns == 0 {
            let ns = now.timestamp_nanos_opt().unwrap_or_default();
            if let Err(err) = self.store.set_alert_pending(rule.id, ns).await {
                tracing::error!(alert = %rule.name, %err, "alerts: start settle timer failed");
                return false;
            }
            tracing::info!(
                alert = %rule.name,
                to = state_name(firing),
                settle_secs = rule.settle_secs,
                "alert condition changed; waiting for it to settle"
            );
            return false;
        }
        let held = now.timestamp_nanos_opt().unwrap_or_default() - rule.pending_since_ns;
        held >= rule.settle_secs.saturating_mul(1_000_000_000)
    }

    async fn clear_pending(&self, rule: &Rule) {
        if let Err(err) = self.store.set_alert_pending(rule.id, 0).await {
            tracing::error!(alert = %rule.name, %err, "alerts: clear settle timer failed");
            return;
        }
        tracing::info!(
            alert = %rule.name,
            settle_secs = rule.settle_secs,
            "alert condition flapped back before settling; not notifying"
        );
    }

    /// POSTs the webhook, logging failures so one bad rule cannot stall the
    /// others in the cycle.
    async fn notify(&self, rule: &Rule, status: &str, detail: &str) {
        // Re-check at use: a rule can predate this guard, or its host can have
        // been re-pointed since creation.
        if let Err(err) = (self.gate)(rule.webhook_url.clone()).await {
            tracing::error!(alert = %rule.name, %err, "alerts: refusing webhook URL");
            return;
        }
        let notification = Notification {
            alert: rule.name.clone(),
            project: rule.project.clone(),
            r#type: rule.r#type.clone(),
            status: status.to_string(),
            detail: detail.to_string(),
            threshold: rule.threshold,
            fired_at: self.now().to_rfc3339_opts(SecondsFormat::Secs, true),
        };
        let payload = webhook_payload(&rule.webhook_url, &notification);
        match post_webhook(&self.client, &rule.webhook_url, &payload, &self.gate).await {
            Ok(()) => tracing::info!(alert = %rule.name, status, "alert notified"),
            Err(err) => tracing::error!(alert = %rule.name, %err, "alerts: webhook post failed"),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
}

fn state_name(firing: bool) -> &'static str {
    if firing { "firing" } else { "resolved" }
}

/// Renders the notification in the shape the destination actually accepts:
/// Slack and Discord ignore arbitrary JSON, so we detect them by host;
/// everything else gets the generic [`Notification`] (for programmatic
/// consumers).
pub fn webhook_payload(webhook_url: &str, notification: &Notification) -> Vec<u8> {
    let firing = notification.status == "firing";
    let line = notification.message();
    let value = match destination(webhook_url) {
        Destination::Slack => serde_json::json!({
            "attachments": [{
                "color": if firing { "#d9534f" } else { "#5cb85c" },
                "text": line,
            }],
        }),
        Destination::Discord => serde_json::json!({
            "embeds": [{
                "description": line,
                "color": if firing { 0xd9534f_u32 } else { 0x5cb85c_u32 },
            }],
        }),
        Destination::Generic => serde_json::to_value(notification).unwrap_or_default(),
    };
    serde_json::to_vec(&value).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Generic,
    Slack,
    Discord,
}

/// Classifies a webhook URL by the host that will receive it.
pub fn destination(webhook_url: &str) -> Destination {
    let Ok(url) = reqwest::Url::parse(webhook_url) else {
        return Destination::Generic;
    };
    let Some(host) = url.host_str() else {
        return Destination::Generic;
    };
    let host = host.to_lowercase();
    if host.contains("hooks.slack.com") {
        Destination::Slack
    } else if host == "discord.com"
        || host == "discordapp.com"
        || host.ends_with(".discord.com")
        || host.ends_with(".discordapp.com")
    {
        Destination::Discord
    } else {
        Destination::Generic
    }
}

impl AlertStore for crate::store::Store {
    async fn list_enabled_alerts(&self) -> Result<Vec<Rule>> {
        crate::store::Store::list_enabled_alerts(self)
    }

    async fn get_stats(&self, filter: &Filter) -> Result<Stats> {
        crate::store::Store::get_stats(self, filter)
    }

    async fn set_alert_firing(&self, id: i64, firing: bool) -> Result<()> {
        crate::store::Store::set_alert_firing(self, id, firing)
    }

    async fn set_alert_pending(&self, id: i64, ns: i64) -> Result<()> {
        crate::store::Store::set_alert_pending(self, id, ns)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::routing::post;

    use super::*;

    #[derive(Default)]
    struct FakeStore {
        rules: Mutex<Vec<Rule>>,
        stats: Mutex<Stats>,
        firing_by_id: Mutex<HashMap<i64, bool>>,
    }

    impl FakeStore {
        fn with_rule(rule: Rule) -> Self {
            Self {
                rules: Mutex::new(vec![rule]),
                ..Self::default()
            }
        }

        fn set_stats(&self, stats: Stats) {
            *self.stats.lock().expect("stats") = stats;
        }

        fn firing(&self) -> bool {
            self.rules.lock().expect("rules")[0].firing
        }
    }

    impl AlertStore for FakeStore {
        async fn list_enabled_alerts(&self) -> Result<Vec<Rule>> {
            Ok(self.rules.lock().expect("rules").clone())
        }

        async fn get_stats(&self, _filter: &Filter) -> Result<Stats> {
            Ok(self.stats.lock().expect("stats").clone())
        }

        async fn set_alert_firing(&self, id: i64, firing: bool) -> Result<()> {
            self.firing_by_id.lock().expect("firing").insert(id, firing);
            for rule in self.rules.lock().expect("rules").iter_mut() {
                if rule.id == id {
                    rule.firing = firing;
                    rule.pending_since_ns = 0;
                }
            }
            Ok(())
        }

        async fn set_alert_pending(&self, id: i64, ns: i64) -> Result<()> {
            for rule in self.rules.lock().expect("rules").iter_mut() {
                if rule.id == id {
                    rule.pending_since_ns = ns;
                }
            }
            Ok(())
        }
    }

    fn error_rule(hook: &str, settle_secs: i64) -> Rule {
        Rule {
            id: 1,
            project: "default".into(),
            name: "errs".into(),
            r#type: "error_rate".into(),
            threshold: 0.2,
            window_secs: 3600,
            webhook_url: hook.into(),
            enabled: true,
            settle_secs,
            ..Rule::default()
        }
    }

    async fn spawn_capture() -> (String, Arc<Mutex<Vec<serde_json::Value>>>, JoinHandle<()>) {
        let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().route(
            "/hook",
            post({
                let received = received.clone();
                move |body: String| {
                    let received = received.clone();
                    async move {
                        if let Ok(value) = serde_json::from_str(&body) {
                            received.lock().expect("received").push(value);
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}/hook"), received, handle)
    }

    fn allow_loopback() -> WebhookGate {
        Arc::new(|_| Box::pin(async { Ok(()) }))
    }

    #[tokio::test]
    async fn fires_and_resolves() {
        let (hook, received, server) = spawn_capture().await;
        let store = Arc::new(FakeStore::with_rule(error_rule(&hook, 0)));
        store.set_stats(Stats {
            runs: 10,
            errors: 5, // 50% > 20% → firing
            ..Stats::default()
        });
        let watcher = Watcher::new(store.clone(), Duration::from_secs(3600))
            .expect("watcher")
            .with_webhook_gate(allow_loopback());

        watcher.evaluate_once().await;
        assert_eq!(received.lock().expect("received").len(), 1);
        assert_eq!(received.lock().expect("received")[0]["status"], "firing");
        assert!(store.firing());

        // Same firing state → no re-fire.
        watcher.evaluate_once().await;
        assert_eq!(received.lock().expect("received").len(), 1);

        // Recover → resolved.
        store.set_stats(Stats {
            runs: 10,
            errors: 0,
            ..Stats::default()
        });
        watcher.evaluate_once().await;
        let got = received.lock().expect("received");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1]["status"], "resolved");
        drop(got);
        server.abort();
    }

    struct Harness {
        watcher: Watcher<FakeStore>,
        store: Arc<FakeStore>,
        now: Arc<Mutex<DateTime<Utc>>>,
        received: Arc<Mutex<Vec<serde_json::Value>>>,
        server: JoinHandle<()>,
    }

    async fn harness(settle_secs: i64) -> Harness {
        let (hook, received, server) = spawn_capture().await;
        let store = Arc::new(FakeStore::with_rule(error_rule(&hook, settle_secs)));
        let now = Arc::new(Mutex::new(
            DateTime::from_timestamp(1_000_000, 0).expect("base time"),
        ));
        let clock = {
            let now = now.clone();
            Arc::new(move || *now.lock().expect("now"))
        };
        let watcher = Watcher::new(store.clone(), Duration::from_secs(3600))
            .expect("watcher")
            .with_webhook_gate(allow_loopback())
            .with_clock(clock);
        Harness {
            watcher,
            store,
            now,
            received,
            server,
        }
    }

    impl Harness {
        async fn tick(&self, by: Duration, errors: i64) {
            {
                let mut now = self.now.lock().expect("now");
                *now += chrono::Duration::from_std(by).expect("duration");
            }
            self.store.set_stats(Stats {
                runs: 10,
                errors,
                ..Stats::default()
            });
            self.watcher.evaluate_once().await;
        }

        fn statuses(&self) -> Vec<String> {
            self.received
                .lock()
                .expect("received")
                .iter()
                .map(|n| n["status"].as_str().unwrap_or_default().to_string())
                .collect()
        }

        fn firing(&self) -> bool {
            self.store.firing()
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    #[tokio::test]
    async fn oscillation_is_silenced_by_settle() {
        let h = harness(300).await;
        for i in 0..10 {
            h.tick(Duration::from_secs(60), if i % 2 == 1 { 0 } else { 5 })
                .await;
        }
        assert_eq!(
            h.statuses().len(),
            0,
            "oscillating metric notified: {:?}",
            h.statuses()
        );
        assert!(!h.firing());
    }

    #[tokio::test]
    async fn oscillation_without_settle_is_noisy() {
        let h = harness(0).await;
        for i in 0..10 {
            h.tick(Duration::from_secs(60), if i % 2 == 1 { 0 } else { 5 })
                .await;
        }
        assert_eq!(h.statuses().len(), 10);
    }

    #[tokio::test]
    async fn persistent_condition_fires_after_settling() {
        let h = harness(300).await;
        h.tick(Duration::from_secs(60), 5).await;
        assert!(h.statuses().is_empty(), "notified before settling");

        h.tick(Duration::from_secs(120), 5).await;
        assert!(h.statuses().is_empty(), "notified after 2 of 5 minutes");

        h.tick(Duration::from_secs(240), 5).await;
        assert_eq!(h.statuses(), vec!["firing".to_string()]);
        assert!(h.firing());

        h.tick(Duration::from_secs(600), 5).await;
        assert_eq!(h.statuses().len(), 1, "re-fired while already firing");
    }

    #[tokio::test]
    async fn recovery_must_also_settle() {
        let h = harness(300).await;
        h.tick(Duration::from_secs(60), 5).await;
        h.tick(Duration::from_secs(360), 5).await;
        assert_eq!(h.statuses(), vec!["firing".to_string()]);

        h.tick(Duration::from_secs(60), 0).await;
        h.tick(Duration::from_secs(60), 5).await;
        h.tick(Duration::from_secs(60), 0).await;
        assert_eq!(h.statuses().len(), 1, "resolved prematurely");
        assert!(h.firing());

        h.tick(Duration::from_secs(360), 0).await;
        assert_eq!(
            h.statuses(),
            vec!["firing".to_string(), "resolved".to_string()]
        );
        assert!(!h.firing());
    }

    #[tokio::test]
    async fn flap_resets_the_settle_timer() {
        let h = harness(300).await;
        h.tick(Duration::from_secs(60), 5).await;
        h.tick(Duration::from_secs(240), 5).await;
        h.tick(Duration::from_secs(60), 0).await;
        h.tick(Duration::from_secs(240), 5).await;
        assert!(
            h.statuses().is_empty(),
            "settle timer accumulated across a flap"
        );
    }

    #[tokio::test]
    async fn stop_cancels_an_in_flight_webhook() {
        let hit = Arc::new(AtomicBool::new(false));
        let app = Router::new().route(
            "/hook",
            post({
                let hit = hit.clone();
                move || {
                    let hit = hit.clone();
                    async move {
                        hit.store(true, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let store = Arc::new(FakeStore::with_rule(error_rule(
            &format!("http://{addr}/hook"),
            0,
        )));
        store.set_stats(Stats {
            runs: 10,
            errors: 10,
            ..Stats::default()
        });
        let mut watcher = Watcher::new(store, Duration::from_millis(10))
            .expect("watcher")
            .with_webhook_gate(allow_loopback());
        watcher.start();

        tokio::time::timeout(Duration::from_secs(5), async {
            while !hit.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("watcher never posted the webhook");

        tokio::time::timeout(Duration::from_secs(2), watcher.stop())
            .await
            .expect("stop blocked on the hung webhook instead of cancelling it");
        server.abort();
    }

    #[test]
    fn payload_shapes_match_go() {
        let notification = Notification {
            alert: "spend".into(),
            project: "default".into(),
            r#type: "cost".into(),
            status: "firing".into(),
            detail: "window cost $6.00 (threshold $5.00)".into(),
            threshold: 5.0,
            fired_at: "2026-11-04T09:00:00Z".into(),
        };

        let slack: serde_json::Value = serde_json::from_slice(&webhook_payload(
            "https://hooks.slack.com/services/T/B/x",
            &notification,
        ))
        .expect("slack json");
        let attachment = &slack["attachments"][0];
        assert_eq!(attachment["color"], "#d9534f");
        let text = attachment["text"].as_str().expect("text");
        assert!(text.contains("firing") && text.contains("spend"), "{text}");

        let discord: serde_json::Value = serde_json::from_slice(&webhook_payload(
            "https://discord.com/api/webhooks/1/abc",
            &notification,
        ))
        .expect("discord json");
        let embed = &discord["embeds"][0];
        assert!(embed["description"].as_str().is_some_and(|d| !d.is_empty()));
        assert_eq!(embed["color"], 0xd9534f_u32);

        let resolved = Notification {
            status: "resolved".into(),
            ..notification.clone()
        };
        let slack: serde_json::Value =
            serde_json::from_slice(&webhook_payload("https://hooks.slack.com/x", &resolved))
                .expect("slack json");
        assert_eq!(slack["attachments"][0]["color"], "#5cb85c");

        let generic: serde_json::Value =
            serde_json::from_slice(&webhook_payload("https://example.com/hook", &notification))
                .expect("generic json");
        assert_eq!(generic["alert"], "spend");
        assert_eq!(generic["status"], "firing");
        assert_eq!(generic["threshold"], 5.0);
        assert_eq!(generic["firedAt"], "2026-11-04T09:00:00Z");
    }

    #[test]
    fn destination_detection() {
        for (url, want) in [
            ("https://hooks.slack.com/services/T/B/x", Destination::Slack),
            (
                "https://discord.com/api/webhooks/1/abc",
                Destination::Discord,
            ),
            (
                "https://discordapp.com/api/webhooks/1/ab",
                Destination::Discord,
            ),
            (
                "https://ptb.discord.com/api/webhooks/1/x",
                Destination::Discord,
            ),
            ("https://example.com/hook", Destination::Generic),
            ("not a url", Destination::Generic),
        ] {
            assert_eq!(destination(url), want, "destination({url:?})");
        }
    }
}
