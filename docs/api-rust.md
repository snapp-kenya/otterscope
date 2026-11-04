# Rust server HTTP API

Contract for the Rust port of the Otterscope server (`rust/crates/otterscope/src/server`).
Everything is JSON with `camelCase` field names unless noted. The one error
envelope is:

```json
{ "error": "message" }
```

All routes live under the configured base path (for example `/otterscope`).
Requests outside it get `404`; the SPA is only served for non-API paths.

## Auth and guard

- `Authorization: Bearer <token>` (read or admin scope) when `-read-auth` is on.
  `GET /api/stream` also accepts `?token=<token>` because EventSource cannot set
  headers.
- Public: `/healthz`, `/api/shared/{token}`, and the SPA.
- Read token: every `GET`. Admin token: every mutating route (assertions, alerts,
  saved views, tokens, evaluate, share minting/revocation).
- `401` for a missing/invalid token on a protected route; `403` when a read token
  reaches an admin route.
- `Origin`/`Host` allow-list on `/api/*` and `/mcp`: loopback, IP literals, and
  hosts named in `OTTERSCOPE_ALLOWED_ORIGINS`. Foreign origins/hosts: `403`.
- Writes with a body must declare `application/json`, else `415`.
- Request bodies are capped at 1 MiB (`413`).
- Every response carries `Content-Security-Policy`, `X-Content-Type-Options:
  nosniff`, `Referrer-Policy: no-referrer`. JSON API responses and CSV exports
  add `Cache-Control: no-store`.
- Unknown `/api/*` paths are `404` JSON; only non-API paths fall back to the SPA.

## Health and metrics

`GET /healthz` → `200`

```json
{ "status": "ok", "version": "0.1.0" }
```

`GET /metrics` → `200 text/plain; version=0.0.4` Prometheus exposition with
`otterscope_build_info`, `otterscope_runs_total`, `otterscope_runs{status=...}`,
`otterscope_steps_total`, `otterscope_projects_total`,
`otterscope_alerts_firing`, `otterscope_db_size_bytes`.

## Runs

`GET /api/runs?limit=&offset=&project=&status=&service=&model=&prompt=&q=&since=&until=`

`limit` defaults to 50 (1..500), `offset` to 0. `since`/`until` are RFC 3339;
a malformed bound is `400`, never silently ignored. Runs are newest-first.

```json
{ "runs": [ {
  "id": "a1b2…", "project": "default", "service": "checkout", "agentName": "support",
  "status": "ok", "start": "2026-08-21T09:00:00.000Z", "durationMs": 1234,
  "inputTokens": 900, "outputTokens": 120, "llmCalls": 3, "toolCalls": 1,
  "models": "claude-sonnet-5", "prompts": "support/v2",
  "costUsd": 0.0123, "costPartial": true, "error": ""
} ] }
```

Omitted when empty: `prompts`, `costUsd`, `costPartial`.

`GET /api/runs/{id}` → `200`

```json
{
  "run": { "…": "same shape as above" },
  "steps": [ {
    "id": "span1", "parentId": "", "kind": "llm", "name": "chat",
    "status": "ok", "start": "2026-08-21T09:00:01.000Z",
    "offsetMs": 1000, "durationMs": 800, "error": "",
    "llm": {
      "provider": "anthropic", "requestModel": "claude-sonnet-5",
      "responseModel": "claude-sonnet-5", "inputTokens": 900,
      "outputTokens": 120, "cacheReadTokens": 0, "reasoningTokens": 0,
      "costUsd": 0.004, "inputMessages": [ { "role": "user", "content": "hi" } ],
      "outputMessages": [ { "role": "assistant", "content": "hello" } ]
    },
    "tool": { "name": "lookup", "callId": "call1", "arguments": "{}", "result": "{}" }
  } ],
  "assertionResults": [
    { "assertionId": 1, "name": "fast", "type": "max_latency_ms", "pass": true, "detail": "" }
  ]
}
```

`llm`/`tool` appear only on the matching step kind. `GET /api/runs/{id}` is
`404` for an unknown run.

`GET /api/runs.csv?…same filters…` → `200 text/csv` with header
`Content-Disposition: attachment; filename="otterscope-runs.csv"`. At most 10,000
rows; when truncated the response carries `X-Otterscope-Truncated: true` and
`X-Otterscope-Row-Limit`.

`GET /api/stats?…same filters…` → `200`

```json
{
  "runs": 120, "errors": 4, "avgDurationMs": 812.5,
  "p50DurationMs": 700, "p95DurationMs": 1900,
  "totalCostUsd": 1.2, "avgCostUsd": 0.01,
  "inputTokens": 90000, "outputTokens": 12000,
  "assertionRates": [ { "assertionId": 1, "name": "fast", "passed": 118, "total": 120 } ]
}
```

`totalCostUsd`/`avgCostUsd` are omitted when no cost is known.

`GET /api/stream` → `text/event-stream`: one `: connected` comment, then
`event: runs` / `data: tick` on each ingest, plus `: ping` every 25 s.

## Assertions and evaluation

`GET /api/assertions?project=` → `200`

```json
{ "assertions": [ { "id": 1, "project": "default", "name": "fast",
                    "type": "max_latency_ms", "config": "20000", "enabled": true } ] }
```

`POST /api/assertions` (admin) — body `{ "project"?, "name", "type", "config" }`;
`enabled` is always true. `201` with the assertion. Invalid type/config: `400`.
Duplicate name in a project: `409`. An `llm_judge` model outside the allow-list:
`400`.

`DELETE /api/assertions/{id}` (admin) → `204`, or `404`.

`POST /api/assertions/evaluate?project=&run=&since=&until=&limit=` (admin)
backfills assertion results. A call must be bounded by `run`, a `since`/`until`
window, or `limit`; otherwise `400`. Unknown run: `404`. Runs are capped
(default 500, lowered by `limit`) and paid judge calls are capped separately
(default 100).

```json
{ "runsEvaluated": 12, "runsSkipped": 3, "runLimit": 15,
  "judgeCalls": 4, "judgeCallsSkipped": 0, "judgeCallLimit": 100 }
```

## Shares

`POST /api/runs/{id}/share` (admin) → `201` — mint a public link.

```json
{ "token": "0123456789abcdef0123456789abcdef", "url": "/otterscope/s/0123…" }
```

`GET /api/runs/{id}/shares` → `200 { "shares": [ { "token": "…", "createdAt": "…" } ] }`.

`DELETE /api/shares/{token}` (admin) → `204`, or `404`.

`GET /api/shared/{token}` → `200` (public, no auth) with `{ "run": …, "steps": […] }`,
or `404` for an unknown/revoked token.

## Saved views

`GET /api/views` → `200 { "views": [ { "id": 1, "name": "prod errors", "params": { "status": "error" } } ] }`

`POST /api/views` (admin) — body `{ "name", "params"? }` (params defaults to `{}`).
`201` with the view; duplicate name `409`; missing name `400`.

`DELETE /api/views/{id}` (admin) → `204`, or `404`.

## Alerts

`GET /api/alerts?project=` → `200`

```json
{ "alerts": [ {
  "id": 1, "project": "default", "name": "errors", "type": "error_rate",
  "threshold": 0.5, "windowSecs": 3600, "config": "",
  "webhookUrl": "https://hooks.slack.com/***",
  "enabled": true, "firing": false, "settleSecs": 0
} ] }
```

The webhook path is always masked to `***` so a Slack/Discord token never leaves
the server.

`POST /api/alerts` (admin) — body `{ "project"?, "name", "type", "threshold",
"windowSecs"?, "config"?, "webhookUrl", "settleSecs"? }`. `windowSecs` defaults
to 3600; `enabled` is true and `firing`/`pendingSinceNs` are watcher-owned.
`201` with the redacted alert. Invalid rule or non-public webhook host: `400`.
Duplicate name: `409`.

`DELETE /api/alerts/{id}` (admin) → `204`, or `404`.

## Tokens

`GET /api/tokens` (admin) → `200`

```json
{ "tokens": [ { "id": "tok_…", "name": "ci", "scope": "read",
                "createdAt": "…", "lastUsedAt": "…" } ] }
```

No secret is ever returned by a listing. `lastUsedAt` is omitted until the token
has been used.

`POST /api/tokens` (admin) — body `{ "name", "scope"? }` where `scope` is
`read` (default) or `admin`. `201`; this is the only response that contains the
secret.

```json
{ "id": "tok_…", "token": "0123456789abcdef0123456789abcdef",
  "name": "ci", "scope": "read", "createdAt": "…" }
```

`DELETE /api/tokens/{id}` (admin) → `204`, or `404`.

## Audit

`GET /api/audit?limit=` (default 200, 1..1000) → `200`

```json
{ "entries": [ { "at": "…", "action": "create", "entity": "view", "detail": "prod errors" } ] }
```

## MCP

`POST /mcp` — MCP Streamable HTTP (JSON-RPC 2.0). Guarded and token-protected
like the API. This build always mounts the MCP router, so the endpoint answers
JSON-RPC: a valid request returns `200` with a JSON-RPC result, and protocol
errors come back as JSON-RPC errors. A build without an MCP router would answer
`501`.
