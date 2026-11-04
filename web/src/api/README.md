# API client contract

The UI talks to the server only through this directory. `contract.ts` holds every
wire type, decoder, endpoint path, enum, and response header name, so adapting to
a future server contract is a change to that one file. `client.ts` owns transport
(auth, error envelopes, JSON), and `index.ts` exposes the typed operations the
query layer uses.

Source of truth: `docs/api-rust.md` (Rust server). All JSON is `camelCase` and
every failure is the one envelope `{ "error": "message" }` — there are no
RFC 9457 problem documents.

## Transport

- Requests go through the build-time base path (`OTTERSCOPE_BASE_PATH`), e.g.
  `/otterscope/api/runs`.
- `Authorization: Bearer <token>` is attached from `localStorage`; a `401`
  prompts once for a token and retries. `GET /api/stream` uses `?token=` because
  EventSource cannot set headers. A `403 admin token required` on share minting
  or revocation prompts once for an admin token, stores it, and retries; a second
  failure is surfaced. Other `403`s are shown without prompting.
- Writes with a body use `Content-Type: application/json`; the server rejects
  other content types with `415` and bodies over 1 MiB with `413`.
- JSON responses carry `Cache-Control: no-store`; the server stamps CSP,
  `X-Content-Type-Options: nosniff`, and `Referrer-Policy: no-referrer`. The UI
  does not add its own security headers.

## Routes

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/healthz` | `{ status, version }` |
| GET | `/metrics` | Prometheus text; client helper only, no screen |
| GET | `/api/runs` | `{ runs }`; `limit` 1..500 (default 50), `offset`; filters below |
| GET | `/api/runs.csv` | `text/csv`; same filters; 10,000-row cap |
| GET | `/api/runs/{id}` | `{ run, steps, assertionResults }`; `404` unknown |
| POST | `/api/runs/{id}/share` | admin; `201 { token, url }` |
| GET | `/api/runs/{id}/shares` | read-level; `{ shares: [{ token, createdAt }] }` |
| DELETE | `/api/shares/{token}` | admin; `204`, or `404` |
| GET | `/api/shared/{token}` | public `{ run, steps }`, or `404` |
| GET | `/api/stats` | same filters; includes `assertionRates` |
| GET | `/api/assertions?project=` | `{ assertions }`; client only |
| POST | `/api/assertions` | admin; body `{ project?, name, type, config }` |
| DELETE | `/api/assertions/{id}` | admin; `204`, or `404` |
| POST | `/api/assertions/evaluate?project=&run=&since=&until=&limit=` | admin; bounded call, returns cap/skip report |
| GET | `/api/views` | `{ views: [{ id, name, params }] }` |
| POST | `/api/views` | admin; `{ name, params? }`; `201`, `409` duplicate |
| DELETE | `/api/views/{id}` | admin; `204`, or `404` |
| GET | `/api/alerts?project=` | `{ alerts }`; webhook paths masked |
| POST | `/api/alerts` | admin; `{ project?, name, type, threshold, windowSecs?, config?, webhookUrl, settleSecs? }` |
| DELETE | `/api/alerts/{id}` | admin; `204`, or `404` |
| GET | `/api/tokens` | admin; `{ tokens }`, never a secret |
| POST | `/api/tokens` | admin; body `{ name, scope? }`; only response with the secret |
| DELETE | `/api/tokens/{id}` | admin; `204`, or `404` |
| GET | `/api/audit?limit=` | limit 1..1000 (default 200); UI asks for 500 |
| GET | `/api/stream` | SSE; `: connected`, `event: runs` / `data: tick`, `: ping` every 25s |
| POST | `/mcp` | MCP Streamable HTTP; `501` when the build has no MCP |

The run-filter query shared by `/api/runs`, `/api/runs.csv`, and `/api/stats` is
`limit`, `offset`, `project`, `status`, `service`, `model`, `prompt`, `q`,
`since`, `until`. `status` is the server enum `running | ok | error`; any other
value is a `400`, not an empty result set. `since`/`until` are RFC 3339; a
malformed bound is a `400`. Runs are newest-first.

The UI pages with `limit = 101` (one lookahead row) and `offset = page * 100`.
CSV truncation is reported by `X-Otterscope-Truncated: true` plus
`X-Otterscope-Row-Limit`.

## Shapes the UI consumes

`Run`, `Step`, `RunDetail`, `Stats`, `SavedView`, `Share`, `Alert`,
`AssertionResult`, `AuditEntry`, `Token`, `Assertion`, and `EvaluateReport`.
`Run.status` and `Step.kind` are the typed server enums. `Stats.assertionRates`
is `{ assertionId, name, passed, total }` — the server already aggregates pass
counts, so the UI never sees per-run `pass`/`fail` there.

Fields omitted when absent: `prompts`, `costUsd`, `costPartial`, step `error`,
step `llm`/`tool`, LLM messages, `pendingSinceNs`, `totalCostUsd`,
`avgCostUsd`, and token `lastUsedAt`. Decoders default optional arrays so a
component never renders `undefined`.

## UI-only decisions

- The run list is polled through SSE plus a 15s safety refresh; run detail is
  polled every 2s and the alerts list every 5s.
- Alert editing is delete + create because the contract has no update route.
  `webhookUrl` comes back masked (`https://host/***`), so the edit dialog
  requires re-entering it.
- No settings screen: assertions, evaluation, tokens, metrics, and MCP are
  exposed by the typed client for completeness, but the UI surface is runs,
  compare, alerts, saved views, audit, and shared runs.
- The server CSP has no `style-src 'unsafe-inline'`. The UI ships no inline
  style attributes: timeline geometry is SVG and the controls are native
  elements, so the production policy produces no console violations.

## Observed server deviation

- `POST /mcp` answers `200` on the current build because MCP is compiled in; the
  documented `501` applies to builds without it.
