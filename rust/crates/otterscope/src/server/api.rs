//! JSON handlers and their wire shapes.
//! All errors use one envelope: `{"error": "..."}`.

use std::collections::HashMap;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::response::Response;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::{
    AlertRecord, ApiBackend, AuditRecord, BackendError, BackendResult, CreatedTokenRecord,
    EvaluateOptions, NewAlert, ShareRecord, StatsRecord, TokenRecord, ViewRecord,
};
use super::{
    ServerState, admin_only, backend_audit, backend_call, backend_option, decode_json,
    insert_header, json_error, json_response, no_content, plain_text, query_int, query_map,
};
use crate::evals::Assertion;
use crate::model::{Run, Status, Step, StepKind};
use crate::store::{Filter, Scope};

pub(crate) fn router() -> Router<ServerState> {
    let admin = || middleware::from_fn(admin_only);
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/runs", get(list_runs))
        .route("/api/runs.csv", get(runs_csv))
        .route("/api/runs/{id}", get(get_run))
        .route(
            "/api/runs/{id}/share",
            post(create_share).route_layer(admin()),
        )
        .route("/api/runs/{id}/shares", get(list_shares))
        .route(
            "/api/shares/{token}",
            delete(delete_share).route_layer(admin()),
        )
        .route("/api/shared/{token}", get(shared_run))
        .route("/api/stats", get(stats))
        .route(
            "/api/assertions",
            get(list_assertions).merge(post(create_assertion).route_layer(admin())),
        )
        .route(
            "/api/assertions/{id}",
            delete(delete_assertion).route_layer(admin()),
        )
        .route(
            "/api/assertions/evaluate",
            post(evaluate).route_layer(admin()),
        )
        .route("/api/audit", get(audit))
        .route(
            "/api/tokens",
            get(list_tokens)
                .route_layer(admin())
                .merge(post(create_token).route_layer(admin())),
        )
        .route(
            "/api/tokens/{id}",
            delete(delete_token).route_layer(admin()),
        )
        .route(
            "/api/views",
            get(list_views).merge(post(create_view).route_layer(admin())),
        )
        .route("/api/views/{id}", delete(delete_view).route_layer(admin()))
        .route(
            "/api/alerts",
            get(list_alerts).merge(post(create_alert).route_layer(admin())),
        )
        .route(
            "/api/alerts/{id}",
            delete(delete_alert).route_layer(admin()),
        )
        .route("/api/stream", get(stream_endpoint))
        .fallback(api_not_found)
        .layer(axum::extract::DefaultBodyLimit::max(super::MAX_BODY_BYTES))
}

/// Unknown API paths are a JSON envelope, never the SPA and never an empty 404.
async fn api_not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "not found")
}

// ---------------------------------------------------------------- wire types

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunJson {
    id: String,
    project: String,
    service: String,
    agent_name: String,
    status: Status,
    start: String,
    duration_ms: i64,
    input_tokens: i64,
    output_tokens: i64,
    llm_calls: i64,
    tool_calls: i64,
    models: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    prompts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "is_false")]
    cost_partial: bool,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StepJson {
    id: String,
    parent_id: String,
    kind: StepKind,
    name: String,
    status: Status,
    start: String,
    offset_ms: i64,
    duration_ms: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    llm: Option<LlmJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<ToolJson>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LlmJson {
    provider: String,
    request_model: String,
    response_model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    reasoning_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input_messages: Vec<crate::model::Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    output_messages: Vec<crate::model::Message>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolJson {
    name: String,
    call_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    arguments: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    result: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsJson {
    runs: i64,
    errors: i64,
    avg_duration_ms: f64,
    p50_duration_ms: i64,
    p95_duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_cost_usd: Option<f64>,
    input_tokens: i64,
    output_tokens: i64,
    assertion_rates: Vec<AssertionRateJson>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssertionRateJson {
    assertion_id: i64,
    name: String,
    passed: i64,
    total: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssertionJson {
    id: i64,
    project: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    config: String,
    enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultJson {
    assertion_id: i64,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    pass: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenJson {
    id: String,
    name: String,
    scope: Scope,
    created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTokenJson {
    id: String,
    token: String,
    name: String,
    scope: Scope,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareJson {
    token: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewJson {
    id: i64,
    name: String,
    params: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditJson {
    at: DateTime<Utc>,
    action: String,
    entity: String,
    detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlertJson {
    id: i64,
    project: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    threshold: f64,
    window_secs: i64,
    config: String,
    webhook_url: String,
    enabled: bool,
    firing: bool,
    settle_secs: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pending_since_ns: i64,
}

#[derive(Debug, Serialize)]
struct RunsBody {
    runs: Vec<RunJson>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunDetailBody {
    run: RunJson,
    steps: Vec<StepJson>,
    assertion_results: Vec<ResultJson>,
}

#[derive(Debug, Serialize)]
struct SharedBody {
    run: RunJson,
    steps: Vec<StepJson>,
}

#[derive(Serialize)]
struct ShareMintBody {
    token: String,
    url: String,
}

#[derive(Serialize)]
struct SharesBody {
    shares: Vec<ShareJson>,
}

#[derive(Debug, Serialize)]
struct ViewsBody {
    views: Vec<ViewJson>,
}

#[derive(Debug, Serialize)]
struct AssertionsBody {
    assertions: Vec<AssertionJson>,
}

#[derive(Debug, Serialize)]
struct TokensBody {
    tokens: Vec<TokenJson>,
}

#[derive(Serialize)]
struct AlertsBody {
    alerts: Vec<AlertJson>,
}

#[derive(Debug, Serialize)]
struct AuditBody {
    entries: Vec<AuditJson>,
}

#[derive(Debug, Serialize)]
struct HealthBody {
    status: &'static str,
    version: String,
}

#[derive(Debug, Deserialize, Default)]
struct TokenInput {
    #[serde(default)]
    name: String,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ViewInput {
    #[serde(default)]
    name: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize, Default)]
struct AssertionInput {
    #[serde(default)]
    project: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    config: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AlertInput {
    #[serde(default)]
    project: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    threshold: f64,
    #[serde(default)]
    window_secs: i64,
    #[serde(default)]
    config: String,
    #[serde(default)]
    webhook_url: String,
    #[serde(default)]
    settle_secs: i64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

// ------------------------------------------------------------- projections

fn run_json(run: &Run) -> RunJson {
    RunJson {
        id: run.id.clone(),
        project: run.project.clone(),
        service: run.service.clone(),
        agent_name: run.agent_name.clone(),
        status: run.status,
        start: format_start(run.start),
        duration_ms: duration_ms(run.start, run.end.unwrap_or(run.start)),
        input_tokens: run.input_tokens,
        output_tokens: run.output_tokens,
        llm_calls: run.llm_calls,
        tool_calls: run.tool_calls,
        models: run.models.clone(),
        prompts: run.prompts.clone(),
        cost_usd: run.cost_usd,
        cost_partial: run.cost_partial,
        error: run.error.clone(),
    }
}

fn format_start(start: DateTime<Utc>) -> String {
    start
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn duration_ms(start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    (end - start).num_milliseconds()
}

fn steps_json(run: &Run, steps: &[Step]) -> Vec<StepJson> {
    steps.iter().map(|step| step_json(run, step)).collect()
}

fn step_json(run: &Run, step: &Step) -> StepJson {
    StepJson {
        id: step.id.clone(),
        parent_id: step.parent_id.clone(),
        kind: step.kind,
        name: step.name.clone(),
        status: step.status,
        start: format_start(step.start),
        offset_ms: duration_ms(run.start, step.start),
        duration_ms: duration_ms(step.start, step.end.unwrap_or(step.start)),
        error: step.error.clone(),
        llm: step.llm.as_ref().map(|llm| LlmJson {
            provider: llm.provider.clone(),
            request_model: llm.request_model.clone(),
            response_model: llm.response_model.clone(),
            input_tokens: llm.input_tokens,
            output_tokens: llm.output_tokens,
            cache_read_tokens: llm.cache_read_tokens,
            reasoning_tokens: llm.reasoning_tokens,
            cost_usd: llm.cost_usd,
            input_messages: llm.input_messages.clone(),
            output_messages: llm.output_messages.clone(),
        }),
        tool: step.tool.as_ref().map(|tool| ToolJson {
            name: tool.name.clone(),
            call_id: tool.call_id.clone(),
            arguments: tool.arguments.clone(),
            result: tool.result.clone(),
        }),
    }
}

fn stats_json(stats: &StatsRecord) -> StatsJson {
    StatsJson {
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
            .iter()
            .map(|rate| AssertionRateJson {
                assertion_id: rate.assertion_id,
                name: rate.name.clone(),
                passed: rate.passed,
                total: rate.total,
            })
            .collect(),
    }
}

fn assertion_json(assertion: &Assertion) -> AssertionJson {
    AssertionJson {
        id: assertion.id,
        project: assertion.project.clone(),
        name: assertion.name.clone(),
        kind: assertion.r#type.clone(),
        config: assertion.config.clone(),
        enabled: assertion.enabled,
    }
}

fn result_json(result: &crate::evals::Result) -> ResultJson {
    ResultJson {
        assertion_id: result.assertion_id,
        name: result.name.clone(),
        kind: result.r#type.clone(),
        pass: result.pass,
        detail: result.detail.clone(),
    }
}

fn token_json(token: &TokenRecord) -> TokenJson {
    TokenJson {
        id: token.id.clone(),
        name: token.name.clone(),
        scope: token.scope,
        created_at: token.created_at,
        last_used_at: token.last_used_at,
    }
}

fn created_token_json(token: &CreatedTokenRecord) -> CreatedTokenJson {
    CreatedTokenJson {
        id: token.id.clone(),
        token: token.token.clone(),
        name: token.name.clone(),
        scope: token.scope,
        created_at: token.created_at,
    }
}

fn share_json(share: &ShareRecord) -> ShareJson {
    ShareJson {
        token: share.token.clone(),
        created_at: share.created_at,
    }
}

fn view_json(view: &ViewRecord) -> ViewJson {
    ViewJson {
        id: view.id,
        name: view.name.clone(),
        params: view.params.clone(),
    }
}

fn audit_json(entry: &AuditRecord) -> AuditJson {
    AuditJson {
        at: entry.at,
        action: entry.action.clone(),
        entity: entry.entity.clone(),
        detail: entry.detail.clone(),
    }
}

fn alert_json(alert: &AlertRecord) -> AlertJson {
    AlertJson {
        id: alert.id,
        project: alert.project.clone(),
        name: alert.name.clone(),
        kind: alert.kind.clone(),
        threshold: alert.threshold,
        window_secs: alert.window_secs,
        config: alert.config.clone(),
        webhook_url: alert.webhook_url.clone(),
        enabled: alert.enabled,
        firing: alert.firing,
        settle_secs: alert.settle_secs,
        pending_since_ns: alert.pending_since_ns,
    }
}

// ---------------------------------------------------------------- responses

fn query_failed(operation: &str, error: BackendError) -> Response {
    tracing::error!(operation, error = %error, "api query failed");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "query failed")
}

fn store_error(operation: &str, error: BackendError, conflict: &str) -> Response {
    match error {
        BackendError::NotFound => json_error(StatusCode::NOT_FOUND, "not found"),
        BackendError::Conflict => json_error(StatusCode::CONFLICT, conflict),
        BackendError::Other(err) => {
            tracing::error!(operation, error = %err, "api store failure");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("{operation} failed"),
            )
        }
    }
}

fn delete_result(operation: &str, result: BackendResult<()>) -> Response {
    match result {
        Ok(()) => no_content(),
        Err(error) => store_error(operation, error, "already exists"),
    }
}

// ---------------------------------------------------------------- handlers

async fn healthz(State(state): State<ServerState>) -> Response {
    json_response(
        StatusCode::OK,
        &HealthBody {
            status: "ok",
            version: state.options.version.clone(),
        },
    )
}

async fn metrics_endpoint(State(state): State<ServerState>) -> Response {
    match backend_call(state.backend.clone(), |backend| backend.metrics()).await {
        Ok(metrics) => {
            let body = super::metrics::render(&state.options.version, &metrics);
            plain_text(
                StatusCode::OK,
                "text/plain; version=0.0.4; charset=utf-8",
                &body,
            )
        }
        Err(err) => {
            tracing::error!(error = %err, "metrics unavailable");
            plain_text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                "metrics unavailable\n",
            )
        }
    }
}

async fn list_runs(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let limit = query_int(&query, "limit", 50, 1, 500);
    let offset = query_int(&query, "offset", 0, 0, 1 << 30);
    let filter = match filter_from(&query) {
        Ok(filter) => filter,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    match backend_call(state.backend.clone(), move |backend| {
        backend.list_runs(&filter, limit, offset)
    })
    .await
    {
        Ok(runs) => json_response(
            StatusCode::OK,
            &RunsBody {
                runs: runs.iter().map(run_json).collect(),
            },
        ),
        Err(err) => query_failed("list runs", err),
    }
}

async fn stats(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let filter = match filter_from(&query) {
        Ok(filter) => filter,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    match backend_call(state.backend.clone(), move |backend| {
        backend.get_stats(&filter)
    })
    .await
    {
        Ok(stats) => json_response(StatusCode::OK, &stats_json(&stats)),
        Err(err) => query_failed("get stats", err),
    }
}

async fn get_run(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let (run, steps) =
        match backend_call(state.backend.clone(), move |backend| backend.get_run(&id)).await {
            Ok(pair) => pair,
            Err(BackendError::NotFound) => {
                return json_error(StatusCode::NOT_FOUND, "run not found");
            }
            Err(err) => return query_failed("get run", err),
        };
    let project = run.project.clone();
    let run_id = run.id.clone();
    let results = match backend_call(state.backend.clone(), move |backend| {
        backend.results_for_run(&project, &run_id)
    })
    .await
    {
        Ok(results) => results,
        Err(err) => return query_failed("results for run", err),
    };
    json_response(
        StatusCode::OK,
        &RunDetailBody {
            run: run_json(&run),
            steps: steps_json(&run, &steps),
            assertion_results: results.iter().map(result_json).collect(),
        },
    )
}

async fn create_share(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let (run, _) =
        match backend_call(state.backend.clone(), move |backend| backend.get_run(&id)).await {
            Ok(pair) => pair,
            Err(BackendError::NotFound) => {
                return json_error(StatusCode::NOT_FOUND, "run not found");
            }
            Err(err) => return query_failed("get run", err),
        };
    let project = run.project;
    let run_id = run.id;
    let token = match backend_call(state.backend.clone(), move |backend| {
        backend.create_share(&project, &run_id)
    })
    .await
    {
        Ok(token) => token,
        Err(err) => {
            tracing::error!(error = %err, "create share");
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create share");
        }
    };
    json_response(
        StatusCode::CREATED,
        &ShareMintBody {
            url: join_base(&state.options.base_path, &format!("/s/{token}")),
            token,
        },
    )
}

async fn list_shares(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let (run, _) =
        match backend_call(state.backend.clone(), move |backend| backend.get_run(&id)).await {
            Ok(pair) => pair,
            Err(BackendError::NotFound) => {
                return json_error(StatusCode::NOT_FOUND, "run not found");
            }
            Err(err) => return query_failed("get run", err),
        };
    let project = run.project;
    let run_id = run.id;
    match backend_call(state.backend.clone(), move |backend| {
        backend.shares_for_run(&project, &run_id)
    })
    .await
    {
        Ok(shares) => json_response(
            StatusCode::OK,
            &SharesBody {
                shares: shares.iter().map(share_json).collect(),
            },
        ),
        Err(err) => query_failed("list shares", err),
    }
}

async fn delete_share(State(state): State<ServerState>, Path(token): Path<String>) -> Response {
    let result = backend_call(state.backend.clone(), move |backend| {
        backend.delete_share(&token)
    })
    .await;
    delete_result("revoke", result)
}

async fn shared_run(State(state): State<ServerState>, Path(token): Path<String>) -> Response {
    let resolved = backend_option(state.backend.clone(), move |backend| {
        backend.resolve_share(&token)
    })
    .await;
    let Some((project, run_id)) = resolved else {
        return json_error(StatusCode::NOT_FOUND, "not found");
    };
    match backend_call(state.backend.clone(), move |backend| {
        backend.get_run_in_project(&project, &run_id)
    })
    .await
    {
        Ok((run, steps)) => json_response(
            StatusCode::OK,
            &SharedBody {
                run: run_json(&run),
                steps: steps_json(&run, &steps),
            },
        ),
        Err(_) => json_error(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn audit(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let limit = query_int(&query, "limit", 200, 1, 1000);
    let entries = backend_audit(state.backend.clone(), move |backend| {
        backend.list_audit(limit)
    })
    .await;
    json_response(
        StatusCode::OK,
        &AuditBody {
            entries: entries.iter().map(audit_json).collect(),
        },
    )
}

async fn list_tokens(State(state): State<ServerState>) -> Response {
    match backend_call(state.backend.clone(), |backend| backend.list_tokens()).await {
        Ok(tokens) => json_response(
            StatusCode::OK,
            &TokensBody {
                tokens: tokens.iter().map(token_json).collect(),
            },
        ),
        Err(err) => query_failed("list tokens", err),
    }
}

async fn create_token(State(state): State<ServerState>, body: Bytes) -> Response {
    let input: TokenInput = match decode_json(&body) {
        Ok(input) => input,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "name is required"),
    };
    if input.name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "name is required");
    }
    let scope = match input.scope.as_deref() {
        None | Some("") | Some("read") => Scope::Read,
        Some("admin") => Scope::Admin,
        Some(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                r#"scope must be "read" or "admin""#,
            );
        }
    };
    let name = input.name;
    match backend_call(state.backend.clone(), move |backend| {
        backend.create_token(&name, scope)
    })
    .await
    {
        Ok(token) => json_response(StatusCode::CREATED, &created_token_json(&token)),
        Err(err) => {
            tracing::error!(error = %err, "create token");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "could not create token")
        }
    }
}

async fn delete_token(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    let result = backend_call(state.backend.clone(), move |backend| {
        backend.delete_token(&id)
    })
    .await;
    delete_result("revoke", result)
}

async fn list_views(State(state): State<ServerState>) -> Response {
    match backend_call(state.backend.clone(), |backend| backend.list_views()).await {
        Ok(views) => json_response(
            StatusCode::OK,
            &ViewsBody {
                views: views.iter().map(view_json).collect(),
            },
        ),
        Err(err) => query_failed("list views", err),
    }
}

async fn create_view(State(state): State<ServerState>, body: Bytes) -> Response {
    let input: ViewInput = match decode_json(&body) {
        Ok(input) => input,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "bad JSON"),
    };
    if input.name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "name is required");
    }
    let params = input
        .params
        .unwrap_or_else(|| Value::Object(Default::default()));
    let name = input.name.clone();
    match backend_call(state.backend.clone(), move |backend| {
        backend.create_view(&name, &params)
    })
    .await
    {
        Ok(view) => json_response(StatusCode::CREATED, &view_json(&view)),
        Err(err) => store_error(
            "create view",
            err,
            &format!("a view named {:?} already exists", input.name),
        ),
    }
}

async fn delete_view(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    match id.parse::<i64>() {
        Ok(id) => {
            let result = backend_call(state.backend.clone(), move |backend| {
                backend.delete_view(id)
            })
            .await;
            delete_result("delete", result)
        }
        Err(_) => json_error(StatusCode::BAD_REQUEST, "bad id"),
    }
}

async fn list_assertions(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let project = query.get("project").cloned().unwrap_or_default();
    match backend_call(state.backend.clone(), move |backend| {
        backend.list_assertions(&project)
    })
    .await
    {
        Ok(assertions) => json_response(
            StatusCode::OK,
            &AssertionsBody {
                assertions: assertions.iter().map(assertion_json).collect(),
            },
        ),
        Err(err) => query_failed("list assertions", err),
    }
}

async fn create_assertion(State(state): State<ServerState>, body: Bytes) -> Response {
    let input: AssertionInput = match decode_json(&body) {
        Ok(input) => input,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "bad JSON"),
    };
    if input.name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "name is required");
    }
    let assertion = Assertion {
        id: 0,
        project: input.project,
        name: input.name,
        r#type: input.kind,
        config: input.config,
        enabled: true,
    };
    if let Err(err) = state.backend.validate_assertion(&assertion) {
        return json_error(StatusCode::BAD_REQUEST, &err);
    }
    if let Ok(model) = state.backend.assertion_judge_model(&assertion)
        && !model.is_empty()
        && let Err(err) = state.backend.check_judge_model(&model)
    {
        return json_error(StatusCode::BAD_REQUEST, &err);
    }
    let to_create = assertion.clone();
    match backend_call(state.backend.clone(), move |backend| {
        backend.create_assertion(to_create)
    })
    .await
    {
        Ok(created) => json_response(StatusCode::CREATED, &assertion_json(&created)),
        Err(err) => store_error(
            "create assertion",
            err,
            &format!(
                "an assertion named {:?} already exists in this project",
                assertion.name
            ),
        ),
    }
}

async fn delete_assertion(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    match id.parse::<i64>() {
        Ok(id) => {
            let result = backend_call(state.backend.clone(), move |backend| {
                backend.delete_assertion(id)
            })
            .await;
            delete_result("delete", result)
        }
        Err(_) => json_error(StatusCode::BAD_REQUEST, "bad id"),
    }
}

async fn evaluate(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let project = query
        .get("project")
        .filter(|project| !project.is_empty())
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    let mut options = EvaluateOptions {
        project: project.clone(),
        run_id: query.get("run").cloned().unwrap_or_default(),
        ..EvaluateOptions::default()
    };
    match parse_bound(query.get("since").map(String::as_str).unwrap_or("")) {
        Ok(bound) => options.since = bound,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &format!("since: {err}")),
    }
    match parse_bound(query.get("until").map(String::as_str).unwrap_or("")) {
        Ok(bound) => options.until = bound,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &format!("until: {err}")),
    }
    if let Some(limit) = query.get("limit").filter(|limit| !limit.is_empty()) {
        match limit.parse::<i64>() {
            Ok(value) if value > 0 => options.limit = value,
            _ => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    &format!("limit must be a positive integer, got {limit:?}"),
                );
            }
        }
    }
    if let Err(err) = super::evaluate::validate(&options) {
        return json_error(StatusCode::BAD_REQUEST, &err.to_string());
    }

    let assertion_project = project.clone();
    let assertions = match backend_call(state.backend.clone(), move |backend| {
        backend.list_assertions(&assertion_project)
    })
    .await
    {
        Ok(assertions) => assertions,
        Err(err) => {
            tracing::error!(project, error = %err, "api: list assertions for evaluate");
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "query failed");
        }
    };
    if let Err(err) = check_judge_models(state.backend.as_ref(), &assertions) {
        return json_error(StatusCode::BAD_REQUEST, &err);
    }

    match state.backend.evaluate(&options).await {
        Ok(report) => json_response(StatusCode::OK, &report),
        Err(BackendError::NotFound) => json_error(StatusCode::NOT_FOUND, "run not found"),
        Err(err) => {
            tracing::error!(project, error = %err, "api: evaluate project");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "evaluation failed")
        }
    }
}

fn check_judge_models(backend: &dyn ApiBackend, assertions: &[Assertion]) -> Result<(), String> {
    for assertion in assertions.iter().filter(|assertion| assertion.enabled) {
        let model = match backend.assertion_judge_model(assertion) {
            Ok(model) => model,
            Err(err) => {
                return Err(format!(
                    "assertion {:?} has an invalid config: {err}",
                    assertion.name
                ));
            }
        };
        if model.is_empty() {
            continue;
        }
        if let Err(err) = backend.check_judge_model(&model) {
            return Err(format!("assertion {:?}: {err}", assertion.name));
        }
    }
    Ok(())
}

async fn list_alerts(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let project = query.get("project").cloned().unwrap_or_default();
    match backend_call(state.backend.clone(), move |backend| {
        backend.list_alerts(&project)
    })
    .await
    {
        Ok(alerts) => json_response(
            StatusCode::OK,
            &AlertsBody {
                alerts: alerts
                    .iter()
                    .map(|alert| {
                        let mut redacted = alert.clone();
                        redacted.webhook_url = state.backend.redact_webhook_url(&alert.webhook_url);
                        alert_json(&redacted)
                    })
                    .collect(),
            },
        ),
        Err(err) => query_failed("list alerts", err),
    }
}

async fn create_alert(State(state): State<ServerState>, body: Bytes) -> Response {
    let input: AlertInput = match decode_json(&body) {
        Ok(input) => input,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "bad JSON"),
    };
    let mut alert = NewAlert {
        project: input.project,
        name: input.name,
        kind: input.kind,
        threshold: input.threshold,
        window_secs: input.window_secs,
        config: input.config,
        webhook_url: input.webhook_url,
        settle_secs: input.settle_secs,
    };
    if alert.window_secs == 0 {
        alert.window_secs = 3600;
    }
    if let Err(err) = state.backend.validate_alert(&alert).await {
        return json_error(StatusCode::BAD_REQUEST, &err);
    }
    let to_create = alert.clone();
    match backend_call(state.backend.clone(), move |backend| {
        backend.create_alert(&to_create)
    })
    .await
    {
        Ok(created) => {
            let mut redacted = created.clone();
            redacted.webhook_url = state.backend.redact_webhook_url(&created.webhook_url);
            json_response(StatusCode::CREATED, &alert_json(&redacted))
        }
        Err(err) => store_error(
            "create alert",
            err,
            &format!(
                "an alert named {:?} already exists in this project",
                alert.name
            ),
        ),
    }
}

async fn delete_alert(State(state): State<ServerState>, Path(id): Path<String>) -> Response {
    match id.parse::<i64>() {
        Ok(id) => {
            let result = backend_call(state.backend.clone(), move |backend| {
                backend.delete_alert(id)
            })
            .await;
            delete_result("delete", result)
        }
        Err(_) => json_error(StatusCode::BAD_REQUEST, "bad id"),
    }
}

async fn stream_endpoint(State(state): State<ServerState>) -> Response {
    axum::response::IntoResponse::into_response(super::stream::response(state.hub.clone()))
}

// ---------------------------------------------------------------- CSV export

async fn runs_csv(State(state): State<ServerState>, RawQuery(raw): RawQuery) -> Response {
    let query = query_map(raw.as_deref());
    let filter = match filter_from(&query) {
        Ok(filter) => filter,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    let cap = usize::try_from(state.options.csv_export_limit).unwrap_or(0);
    let fetch = i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX);
    let runs = match backend_call(state.backend.clone(), move |backend| {
        backend.list_runs(&filter, fetch, 0)
    })
    .await
    {
        Ok(runs) => runs,
        Err(err) => return query_failed("export runs", err),
    };
    let truncated = runs.len() > cap;
    let runs = &runs[..runs.len().min(cap)];

    let mut csv = String::new();
    write_csv_row(
        &mut csv,
        &[
            "id",
            "project",
            "service",
            "agent",
            "status",
            "start",
            "duration_ms",
            "input_tokens",
            "output_tokens",
            "llm_calls",
            "tool_calls",
            "models",
            "prompts",
            "cost_usd",
            "error",
        ],
    );
    for run in runs {
        let run = run_json(run);
        let cost = run.cost_usd.map(format_cost).unwrap_or_default();
        write_csv_row(
            &mut csv,
            &[
                &run.id,
                &run.project,
                &run.service,
                &run.agent_name,
                run.status.as_str(),
                &run.start,
                &run.duration_ms.to_string(),
                &run.input_tokens.to_string(),
                &run.output_tokens.to_string(),
                &run.llm_calls.to_string(),
                &run.tool_calls.to_string(),
                &run.models,
                &run.prompts,
                &cost,
                &run.error,
            ],
        );
    }

    let mut response = plain_text(StatusCode::OK, "text/csv; charset=utf-8", &csv);
    insert_header(response.headers_mut(), header::CACHE_CONTROL, "no-store");
    insert_header(
        response.headers_mut(),
        header::CONTENT_DISPOSITION,
        "attachment; filename=\"otterscope-runs.csv\"",
    );
    if truncated {
        insert_header(
            response.headers_mut(),
            header::HeaderName::from_static("x-otterscope-truncated"),
            "true",
        );
        insert_header(
            response.headers_mut(),
            header::HeaderName::from_static("x-otterscope-row-limit"),
            &cap.to_string(),
        );
        insert_header(
            response.headers_mut(),
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "X-Otterscope-Truncated, X-Otterscope-Row-Limit",
        );
    }
    response
}

/// Fixed decimals at the same thresholds as the UI, so CSV consumers never see
/// a float artefact such as `0.0050311999999999996`.
fn format_cost(cost: f64) -> String {
    if cost >= 1.0 {
        format!("{cost:.2}")
    } else if cost >= 0.01 {
        format!("{cost:.3}")
    } else {
        format!("{cost:.5}")
    }
}

fn write_csv_row(out: &mut String, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&csv_field(field));
    }
    out.push('\n');
}

fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

// ---------------------------------------------------------------- filters

fn parse_status(raw: &str) -> Result<Option<Status>, String> {
    match raw {
        "" => Ok(None),
        "running" => Ok(Some(Status::Running)),
        "ok" => Ok(Some(Status::Ok)),
        "error" => Ok(Some(Status::Error)),
        other => Err(format!(
            "unknown status {other:?} (expected running, ok or error)"
        )),
    }
}

fn filter_from(query: &HashMap<String, String>) -> Result<Filter, String> {
    let get = |key: &str| query.get(key).cloned().unwrap_or_default();
    let status = parse_status(&get("status"))?;
    let mut filter = Filter {
        project: get("project"),
        status: status
            .map(|status| status.as_str().to_string())
            .unwrap_or_default(),
        service: get("service"),
        model: get("model"),
        prompt: get("prompt"),
        query: get("q"),
        since: None,
        until: None,
    };
    filter.since = parse_bound(&get("since")).map_err(|err| format!("since: {err}"))?;
    filter.until = parse_bound(&get("until")).map_err(|err| format!("until: {err}"))?;
    Ok(filter)
}

/// An empty bound means "no constraint"; a malformed one is an error rather
/// than a silently dropped constraint.
fn parse_bound(value: &str) -> Result<Option<DateTime<Utc>>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    DateTime::parse_from_rfc3339(value)
        .map(|bound| Some(bound.with_timezone(&Utc)))
        .map_err(|_| format!("{value:?} is not an RFC 3339 timestamp (e.g. 2026-08-21T09:00:00Z)"))
}

fn join_base(base_path: &str, path: &str) -> String {
    if base_path.is_empty() {
        path.to_string()
    } else {
        format!("{base_path}{path}")
    }
}
