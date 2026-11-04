//! Production [`ApiBackend`]: store calls plus the evals, alerts and MCP
//! surfaces the HTTP layer needs.
//!
//! This is the one file that knows the concrete modules, which keeps their API
//! churn out of the handlers. It is compiled by `cargo build`/`check` (not by
//! `cargo test`, which substitutes the in-memory backend).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::Options;
use super::backend::{
    AlertRecord, ApiBackend, BackendError, BackendResult, CreatedTokenRecord, EvaluateOptions,
    EvaluateReport, NewAlert, ShareRecord, StatsRecord, TokenRecord, ViewRecord, map_store_error,
};
use crate::evals::{self, Assertion};
use crate::model::{Run, Step};
use crate::store::{Filter, Metrics, Scope, Store};

pub(crate) struct RealBackend {
    store: Arc<Store>,
    judge: evals::Endpoint,
    version: String,
}

impl RealBackend {
    pub fn new(store: Arc<Store>, options: &Options) -> Self {
        Self {
            store,
            judge: evals::Endpoint {
                base_url: options.judge_url.clone(),
                key: options.judge_key.clone(),
                models: options.judge_models.clone(),
            },
            version: options.version.clone(),
        }
    }
}

fn require<T>(value: Option<T>) -> BackendResult<T> {
    value.ok_or(BackendError::NotFound)
}

fn require_deleted(deleted: bool) -> BackendResult<()> {
    if deleted {
        Ok(())
    } else {
        Err(BackendError::NotFound)
    }
}

fn alert_rule(alert: &NewAlert) -> crate::alerts::Rule {
    crate::alerts::Rule {
        id: 0,
        project: alert.project.clone(),
        name: alert.name.clone(),
        r#type: alert.kind.clone(),
        threshold: alert.threshold,
        window_secs: alert.window_secs,
        config: alert.config.clone(),
        webhook_url: alert.webhook_url.clone(),
        enabled: true,
        firing: false,
        settle_secs: alert.settle_secs,
        pending_since_ns: 0,
    }
}

fn alert_record(rule: crate::alerts::Rule) -> AlertRecord {
    AlertRecord {
        id: rule.id,
        project: rule.project,
        name: rule.name,
        kind: rule.r#type,
        threshold: rule.threshold,
        window_secs: rule.window_secs,
        config: rule.config,
        webhook_url: rule.webhook_url,
        enabled: rule.enabled,
        firing: rule.firing,
        settle_secs: rule.settle_secs,
        pending_since_ns: rule.pending_since_ns,
    }
}

impl ApiBackend for RealBackend {
    fn list_runs(&self, filter: &Filter, limit: i64, offset: i64) -> BackendResult<Vec<Run>> {
        self.store
            .list_runs(filter, limit, offset)
            .map_err(map_store_error)
    }

    fn get_run(&self, id: &str) -> BackendResult<(Run, Vec<Step>)> {
        let run = self.store.get_run(id).map_err(map_store_error)?;
        require(run)
    }

    fn get_run_in_project(&self, project: &str, id: &str) -> BackendResult<(Run, Vec<Step>)> {
        let run = self
            .store
            .get_run_in_project(project, id)
            .map_err(map_store_error)?;
        require(run)
    }

    fn get_stats(&self, filter: &Filter) -> BackendResult<StatsRecord> {
        let stats = self.store.get_stats(filter).map_err(map_store_error)?;
        // The store's rate rows drop the assertion id; the UI keys on it, so
        // recover it by name from the project's assertions.
        let assertions = self
            .store
            .list_assertions(&filter.project)
            .unwrap_or_default();
        Ok(StatsRecord {
            runs: stats.runs,
            errors: stats.errors,
            avg_duration_ms: stats.avg_duration_ms,
            p50_duration_ms: stats.p50_duration_ms,
            p95_duration_ms: stats.p95_duration_ms,
            total_cost_usd: stats.total_cost_usd,
            avg_cost_usd: stats.avg_cost_usd,
            input_tokens: stats.input_tokens,
            output_tokens: stats.output_tokens,
            assertion_rates: stats
                .assertion_rates
                .into_iter()
                .map(|rate| super::backend::AssertionRateRecord {
                    assertion_id: assertions
                        .iter()
                        .find(|assertion| assertion.name == rate.name)
                        .map(|assertion| assertion.id)
                        .unwrap_or(0),
                    name: rate.name,
                    passed: rate.pass,
                    total: rate.pass + rate.fail,
                })
                .collect(),
        })
    }

    fn metrics(&self) -> BackendResult<Metrics> {
        self.store.metrics().map_err(map_store_error)
    }

    fn list_assertions(&self, project: &str) -> BackendResult<Vec<Assertion>> {
        self.store.list_assertions(project).map_err(map_store_error)
    }

    fn create_assertion(&self, assertion: Assertion) -> BackendResult<Assertion> {
        self.store
            .create_assertion(assertion)
            .map_err(map_store_error)
    }

    fn delete_assertion(&self, id: i64) -> BackendResult<()> {
        let deleted = self.store.delete_assertion(id).map_err(map_store_error)?;
        require_deleted(deleted)
    }

    fn results_for_run(
        &self,
        project: &str,
        run_id: &str,
    ) -> BackendResult<Vec<crate::evals::Result>> {
        self.store
            .results_for_run(project, run_id)
            .map_err(map_store_error)
    }

    fn evaluate<'a>(
        &'a self,
        options: &'a EvaluateOptions,
    ) -> Pin<Box<dyn Future<Output = BackendResult<EvaluateReport>> + Send + 'a>> {
        Box::pin(async move {
            super::evaluate::evaluate_project(&self.store, &self.judge, options)
                .await
                .map_err(map_store_error)
        })
    }

    fn list_alerts(&self, project: &str) -> BackendResult<Vec<AlertRecord>> {
        let rules = self.store.list_alerts(project).map_err(map_store_error)?;
        Ok(rules.into_iter().map(alert_record).collect())
    }

    fn create_alert(&self, alert: &NewAlert) -> BackendResult<AlertRecord> {
        let created = self
            .store
            .create_alert(alert_rule(alert))
            .map_err(map_store_error)?;
        Ok(alert_record(created))
    }

    fn delete_alert(&self, id: i64) -> BackendResult<()> {
        let deleted = self.store.delete_alert(id).map_err(map_store_error)?;
        require_deleted(deleted)
    }

    fn create_share(&self, project: &str, run_id: &str) -> BackendResult<String> {
        self.store
            .create_share(project, run_id)
            .map_err(map_store_error)
    }

    fn resolve_share(&self, token: &str) -> Option<(String, String)> {
        self.store.resolve_share(token).ok().flatten()
    }

    fn shares_for_run(&self, project: &str, run_id: &str) -> BackendResult<Vec<ShareRecord>> {
        let shares = self
            .store
            .shares_for_run(project, run_id)
            .map_err(map_store_error)?;
        Ok(shares
            .into_iter()
            .map(|share| ShareRecord {
                token: share.token,
                created_at: share.created_at,
            })
            .collect())
    }

    fn delete_share(&self, token: &str) -> BackendResult<()> {
        let deleted = self.store.delete_share(token).map_err(map_store_error)?;
        require_deleted(deleted)
    }

    fn list_views(&self) -> BackendResult<Vec<ViewRecord>> {
        let views = self.store.list_saved_views().map_err(map_store_error)?;
        Ok(views
            .into_iter()
            .map(|view| ViewRecord {
                id: view.id,
                name: view.name,
                params: view.params,
            })
            .collect())
    }

    fn create_view(&self, name: &str, params: &serde_json::Value) -> BackendResult<ViewRecord> {
        let view = self
            .store
            .create_saved_view(name, params)
            .map_err(map_store_error)?;
        Ok(ViewRecord {
            id: view.id,
            name: view.name,
            params: view.params,
        })
    }

    fn delete_view(&self, id: i64) -> BackendResult<()> {
        let deleted = self.store.delete_saved_view(id).map_err(map_store_error)?;
        require_deleted(deleted)
    }

    fn list_audit(&self, limit: i64) -> Vec<super::backend::AuditRecord> {
        self.store
            .list_audit(limit)
            .into_iter()
            .map(|entry| super::backend::AuditRecord {
                at: entry.at,
                action: entry.action,
                entity: entry.entity,
                detail: entry.detail,
            })
            .collect()
    }

    fn list_tokens(&self) -> BackendResult<Vec<TokenRecord>> {
        let tokens = self.store.list_tokens().map_err(map_store_error)?;
        Ok(tokens
            .into_iter()
            .map(|token| TokenRecord {
                id: token.id,
                name: token.name,
                scope: token.scope,
                created_at: token.created_at,
                last_used_at: token.last_used_at,
            })
            .collect())
    }

    fn create_token(&self, name: &str, scope: Scope) -> BackendResult<CreatedTokenRecord> {
        let token = self
            .store
            .create_token(name, scope)
            .map_err(map_store_error)?;
        Ok(CreatedTokenRecord {
            id: token.id,
            token: token.token,
            name: token.name,
            scope: token.scope,
            created_at: token.created_at,
        })
    }

    fn delete_token(&self, id: &str) -> BackendResult<()> {
        let deleted = self.store.delete_token(id).map_err(map_store_error)?;
        require_deleted(deleted)
    }

    fn authenticate_token(&self, token: &str) -> Option<TokenRecord> {
        let token = self.store.authenticate_token(token).ok().flatten()?;
        Some(TokenRecord {
            id: token.id,
            name: token.name,
            scope: token.scope,
            created_at: token.created_at,
            last_used_at: token.last_used_at,
        })
    }

    fn validate_assertion(&self, assertion: &Assertion) -> Result<(), String> {
        evals::validate(assertion).map_err(|err| err.to_string())
    }

    fn assertion_judge_model(&self, assertion: &Assertion) -> Result<String, String> {
        assertion.judge_model().map_err(|err| err.to_string())
    }

    fn check_judge_model(&self, model: &str) -> Result<(), String> {
        self.judge.check_model(model).map_err(|err| err.to_string())
    }

    fn validate_alert<'a>(
        &'a self,
        alert: &'a NewAlert,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            crate::alerts::validate(&alert_rule(alert))
                .await
                .map_err(|err| err.to_string())
        })
    }

    fn redact_webhook_url(&self, raw: &str) -> String {
        crate::alerts::redact_webhook_url(raw)
    }

    fn mcp_router(&self) -> Option<axum::Router> {
        let store = crate::mcp::BlockingStore::new(self.store.clone());
        Some(crate::mcp::Handler::new(Arc::new(store), self.version.clone()).into_router())
    }
}
