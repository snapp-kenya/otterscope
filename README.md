# Otterscope

**Lightweight, self-hosted observability and evals for AI agents.**

One static binary. One SQLite file. No ClickHouse, no Redis, no S3, no seat fees, no trace metering. Point your agent's OpenTelemetry exporter at Otterscope and get an agent-run-first view of everything it did — LLM calls, tool calls, loops, errors, latency, and cost — plus assertions and LLM-as-judge evals scored onto your real production runs. All on your own disk, with unlimited retention.

> Think *Plausible Analytics, but for AI agents*.

![Runs list — live-tailing agent runs with status, duration, models, tokens, and cost](docs/screenshots/runs-list.png)

## Quick start

```sh
# grab a binary from Releases, then:
./otterscope serve
```

By default the UI and ingest bind to **loopback** (`127.0.0.1`) — safe on a
shared machine. To expose them on your network, pass `-listen :8317 -otlp :4318`
and add `-read-auth`; without it the API is a full read/write surface for
anyone who can reach the port.
For reproducible deployments, put it all in a config file — `serve -config otterscope.json`
(see [docs/otterscope.example.json](docs/otterscope.example.json)) — or `OTTERSCOPE_*`
env vars; flags override env override the file.

To serve everything (UI, API, `/mcp`, share links, and the OTLP receiver) under
one path prefix — for instance behind an ingress that routes by path rather
than hostname — pass `-base-path /otterscope` (`OTTERSCOPE_BASE_PATH`, or
`"basePath"` in the config file; one setting whichever source you use). Point
exporters at `http://host:4318/otterscope`; OTLP/HTTP appends
`/v1/traces` itself, so ingest lands at `/otterscope/v1/traces`. Build the UI
with the same value (`OTTERSCOPE_BASE_PATH=/otterscope npm --prefix web run
build`) so its asset and API URLs carry the prefix. The default is the root,
and a request outside the prefix is a 404 rather than the SPA, so a misrouted
path cannot land on another application's index.

Auth is off by default, which is the right posture on loopback. Before you
expose the instance, pass `-read-auth`: the UI and API then need a Bearer
token. Mint the first one with `otterscope token add local` — it is printed
once and stored only as a SHA-256 digest; `-admin` mints a token that may
also mutate. Read tokens cover the query surface; minting or revoking tokens,
writing assertions, saved views or alerts, and on-demand evaluation need an
admin token. `otterscope token list` shows ids, scopes and last use, never a
secret. With read auth on, OTLP ingest also requires a project ingest key
(`otterscope project add my-agent`); the keyless default project is closed.
If the UI is reached by a hostname through a reverse proxy, list that origin
in `OTTERSCOPE_ALLOWED_ORIGINS` — the API checks `Origin` and `Host` on every
request and refuses a name it was not told about.

Docker works the same way. The image is a scratch build running as uid
65532, with `/data` as its only writable path; it keeps the binary's loopback
defaults, so pass `-listen`/`-otlp` to let Docker's port forwarding reach it,
and publish only to your loopback:

```sh
docker run --read-only -p 127.0.0.1:8317:8317 -p 127.0.0.1:4318:4318 \
  -v otterscope:/data ghcr.io/otterscope/otterscope \
  serve -db /data/otterscope.db -listen 0.0.0.0:8317 -otlp 0.0.0.0:4318
```

[docker-compose.yml](docker-compose.yml) is the maintained version of that
command, with read auth on and the UI published to 127.0.0.1 only. A named
volume inherits `/data`'s ownership from the image; a host bind mount has to
be owned by 65532.

Point any OTel-instrumented agent at it:

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
```

Open **http://localhost:8317**. No account by default, no config file, no
other services.

Ingest is bounded. One OTLP request may carry at most 10,000 spans and touch
at most 1,000 distinct traces (`-ingest-max-spans`, `-ingest-max-trace-ids`);
a larger batch is refused whole with a 400 before anything is normalized. One
ingest key may send 50 batches a second by default (`-ingest-rate`,
`-ingest-burst`). A span id that already belongs to another service is
rejected and reported in OTLP partial success instead of overwriting that
service's step.

Want to poke around before wiring an agent? `./otterscope sample` seeds realistic demo runs.

Want your agent to inspect *itself*? Otterscope ships an [MCP server](docs/mcp.md), so Claude Code and other MCP clients can query your runs, cost, and stats:

```sh
claude mcp add otterscope --transport http http://localhost:8317/mcp
```

## What you get

- **Runs, not span soup** — every trace becomes an agent run: steps, tool loops, per-run tokens and cost, error surfacing, live-tailing list with filters (status, model, service, project, time) and full-text search over message and tool content — all URL-shareable, and saveable as named views ("prod errors 24h") you can pin and reapply.
- **Run inspector** — step timeline with proportional duration bars; click any LLM call to read the actual messages in/out, token breakdowns (cache reads, reasoning), and cost; tool calls show arguments and results. Share any run as a public read-only link (opt-in per run, revocable) to hand a teammate a broken run without giving them your whole instance.

  ![Run detail — step timeline, assertion chips, and message inspector](docs/screenshots/run-detail.png)
- **Cost tracking** — maintained pricing table for major providers (override or extend with `serve -pricing yours.json`); unknown models show tokens, never fabricated costs.
- **Evals fused into the trace store** — assertions (`contains`, `regex`, `is_json`, latency/cost thresholds) and **LLM-as-judge** scored onto real runs at ingest or backfilled on demand. The judge endpoint, key, and model allow-list are server config (`-judge-url`, `OTTERSCOPE_JUDGE_KEY`, `-judge-models`), never per-assertion — so assertions can't name secrets to read or an arbitrary model to bill. On-demand backfills (`POST /api/assertions/evaluate`) must name a run, a time window, or a limit, and are capped per call (500 runs, 100 judge calls by default) with the skipped counts reported. No second product.
- **Compare view** — error rate, p50/p95 latency, cost, and assertion pass rates side-by-side across any two filters: this week vs last, model A vs model B, or **prompt version v2 vs v3** (captured from `gen_ai.prompt.name`/`version`). *"Did my prompt change make things worse?"* is one URL.
- **Alerting** — rules on error rate, cost, p95 latency, or an assertion's fail rate over a window; Otterscope POSTs a JSON notification to your webhook (Slack/Discord/…) when a rule starts firing and again when it resolves. A settle time (default 5 minutes) requires the condition to hold before either notification, so a value sitting on the threshold can't page you twice a minute. Webhook URLs must be public HTTP(S) endpoints: loopback, link-local and private ranges are refused on creation and again on every POST, including through redirects, and a stored URL reads back redacted.

  ![Alerts — rules on error rate, cost, latency, or assertion failures firing to a webhook](docs/screenshots/alerts.png)

  ![Compare — two models side by side: error rate, latency percentiles, cost, assertion pass rates](docs/screenshots/compare.png)
- **Drop-in OTel compatibility** — normalizes the OTel GenAI conventions (both current dialects), OpenInference (OpenAI Agents SDK, CrewAI, LangChain), and the Vercel AI SDK. Raw payloads are retained, so old data benefits from future normalizer improvements.
- **Ask your agent about itself** — a built-in [MCP server](docs/mcp.md) (`POST /mcp`) lets Claude Code and other MCP clients query your runs, steps, cost, and stats; it needs the same read token when `-read-auth` is on. `claude mcp add otterscope --transport http http://localhost:8317/mcp`.
- **Projects + ingest keys** for isolating multiple agents; **API tokens** scoped read or admin (`otterscope token add`, `-admin` for one that may mutate) to script against your data or build external dashboards — enforce them with `serve -read-auth` when you expose the instance, which also makes an ingest key mandatory. Optional retention sweep (`-retention 720h`) if you *want* to delete data; it removes the full-text index rows with the runs and vacuums, so deleted content stops being searchable.
- **Backups you can trust** — `otterscope backup -o snapshot.db` for a consistent copy (0600, like the database), and Otterscope auto-snapshots before applying any schema migration, keeping the newest one, so an upgrade can never leave you stranded. The database, its `-wal`/`-shm` siblings and a directory Otterscope creates for them are owner-only too.
- **Prometheus metrics** at `/metrics` (runs, statuses, steps, DB size including WAL and SHM, firing alerts) — the observability tool is itself observable. With `-read-auth` it takes the same read token as the API.
- **Export your data** — download the current filtered run set as CSV, or script the read API with a token. No lock-in.
- **Audit log** — every create/delete of assertions, alerts, projects, tokens, views, and shares, from the UI, API, or CLI.

  ![Audit log](docs/screenshots/audit.png)

## Connecting your framework

Guides for [Pydantic AI](docs/frameworks/pydantic-ai.md), [OpenAI Agents SDK](docs/frameworks/openai-agents.md), [Vercel AI SDK](docs/frameworks/vercel-ai-sdk.md), [LangGraph](docs/frameworks/langgraph.md), [Claude Agent SDK / Claude Code](docs/frameworks/claude-agent-sdk.md), and [anything else that speaks OTLP](docs/frameworks/generic-otlp.md).

## Why not Langfuse / LangSmith / Phoenix?

They're good products aimed at a different deployment reality:

- **Langfuse** self-hosting requires Postgres + ClickHouse + Redis + S3 — six containers to log a few thousand LLM calls a day.
- **LangSmith** and **Braintrust** gate self-hosting behind enterprise contracts.
- **Phoenix** is ELv2-licensed with an upsell funnel.

Otterscope is Apache-2.0, installs in one command, and is built for individuals and small teams whose traces contain customer data they'd rather keep on their own disk. There is no paid tier, no hosted version, no metering and no seat fees — every feature is in the binary you just downloaded ([ADR-0005](docs/adr/0005-no-hosted-or-paid-offering.md)).

## Install

- **Binaries** — [Releases](https://github.com/otterscope/otterscope/releases) (Linux amd64/arm64, macOS, Windows) + `.deb`/`.rpm`
- **Homebrew** — `brew install otterscope/tap/otterscope`
- **Docker** — `ghcr.io/otterscope/otterscope`
- **Go** — `go install github.com/otterscope/otterscope/cmd/otterscope@latest`

## Development

Go backend, embedded React UI. `go build ./...` works without building the frontend; `cd web && npm run build` embeds the real UI. See [CLAUDE.md](CLAUDE.md) for architecture and [docs/WORKFLOW.md](docs/WORKFLOW.md) for the contribution process.

Contributions are welcome and there's nothing to sign — no CLA, no copyright assignment. Issues tagged [good first issue](https://github.com/otterscope/otterscope/labels/good%20first%20issue) are scoped small on purpose: each one names the file and line, what's wrong, and what "done" looks like.

## License

[Apache-2.0](LICENSE)
