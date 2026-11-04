# API client contract

The UI talks to the server only through this directory. `contract.ts` holds every
wire shape, decoder, endpoint path, and response header name the UI assumes, so
porting to the Rust server's published contract is a one-file change there.

## Endpoints assumed

| Method | Path | Response |
| --- | --- | --- |
| GET | `/healthz` | `{ status, version }` |
| GET | `/api/runs` | `{ runs: Run[] }` |
| GET | `/api/runs.csv` | `text/csv`; `X-Otterscope-Truncated`, `X-Otterscope-Row-Limit` |
| GET | `/api/runs/{id}` | `{ run, steps, assertionResults? }` |
| POST | `/api/runs/{id}/share` | `{ token, url }` |
| GET | `/api/runs/{id}/shares` | `{ shares: [{ token, createdAt }] }` |
| DELETE | `/api/shares/{token}` | `204` |
| GET | `/api/shared/{token}` | `{ run, steps }` |
| GET | `/api/stats` | `Stats` |
| GET | `/api/views` | `{ views: [{ id, name, params }] }` |
| POST | `/api/views` | `{ id, name, params }` from `{ name, params }` |
| DELETE | `/api/views/{id}` | `204` |
| GET | `/api/alerts` | `{ alerts: Alert[] }` |
| POST | `/api/alerts` | `Alert` from `AlertInput` |
| DELETE | `/api/alerts/{id}` | `204` |
| GET | `/api/audit?limit=` | `{ entries: [{ at, action, entity, detail }] }` |
| GET | `/api/stream` | SSE; event `runs` means "refetch the run list" |

`/api/runs`, `/api/runs.csv`, and `/api/stats` share the run-filter query:
`limit`, `offset`, `q`, `project`, `status`, `service`, `model`, `prompt`,
`since`, `until`. The client pages with `limit = 101` (one lookahead row) and
`offset = page * 100`.

Every timestamp is an RFC 3339 string. Run detail is polled every 2 seconds and
the run list is invalidated by the SSE stream. Authenticated reads carry
`Authorization: Bearer <token>`; the stream carries the token as `?token=`.

## Shapes

The UI-facing types in `contract.ts` (`Run`, `Step`, `RunDetail`, `Stats`,
`Alert`, `SavedView`, `Share`, `AuditEntry`) are what components consume. The
`*Wire` types and `decode*` functions sit between them and the server, defaulting
optional arrays so components never render `undefined`.
