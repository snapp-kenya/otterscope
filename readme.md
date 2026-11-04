# Otterscope

**Lightweight, self-hosted observability and evals for AI agents.**

One self-contained binary. One SQLite file. No ClickHouse, no Redis, no S3, no seat fees, no trace metering. Point your agent's OpenTelemetry exporter at Otterscope and get an agent-run-first view of everything it did — LLM calls, tool calls, loops, errors, latency, and cost — plus assertions and LLM-as-judge evals scored onto your real production runs. All on your own disk, with unlimited retention.

> Think *Plausible Analytics, but for AI agents*.

![Runs list — live-tailing agent runs with status, duration, models, tokens, and cost](docs/screenshots/runs-list.png)

## Quick start

Otterscope's only deployment target is **Kubernetes via k3d on the operator's
MacBook Pro**. Nothing is published: no image, no archive, no package. The
image is built from this checkout, tagged with the short git SHA, imported into
the local k3d cluster, and never pushed anywhere.

Prerequisites: Docker Desktop with the daemon running, `k3d`, `kubectl`, and a
checkout of this repository. Rust and Bun are not needed on the host; the build
runs in Docker.

```sh
# From the repository root. Refuses a dirty tree, builds HEAD, creates the k3d
# cluster if it is missing, imports the image and waits for the rollout.
deploy/k8s/deploy.sh

# In another shell: publish the Service on localhost.
kubectl -n otterscope port-forward svc/otterscope 8080:80
```

The Deployment runs `otterscope:<short-sha>`; the script prints that tag. Check
it through the port-forward:

```sh
curl -fsS http://127.0.0.1:8080/healthz
# {"status":"ok","version":"<short-sha>"}
```

Open **http://localhost:8080**. Mint the first credentials against the live
database inside the server pod — the secrets are printed once:

```sh
# Admin token: mint/revoke tokens, assertions, views, alerts, on-demand evals.
kubectl -n otterscope exec deploy/otterscope -- otterscope token add admin -admin

# Read token: the UI/dashboard query surface.
kubectl -n otterscope exec deploy/otterscope -- otterscope token add ui

# Project + ingest key for agents.
kubectl -n otterscope exec deploy/otterscope -- otterscope project add agents
```

Paste the read token into the UI, or send it as `Authorization: Bearer <token>`.
`otterscope token list` and `project list` never print secrets again. The full
runbook (upgrade, rollback, backup, teardown) is
[deploy/readme.md](deploy/readme.md).

Point an OTel-instrumented agent **in the cluster** at the Service. Port 80 is
implied and the exporter appends `/v1/traces` itself:

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://otterscope.otterscope.svc.cluster.local
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <ingest key>"
```

For an agent on the laptop, use the port-forward instead (same key):

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:8080
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <ingest key>"
```

The server has **one HTTP listener, always on port 80**. The UI, JSON API,
`/mcp`, `/healthz`, share links and OTLP ingest (`POST /v1/traces`) all live
there. The Kubernetes Service exposes port 80; TLS, hostnames and any external
routing are the cluster's business, not the application's.

Port 80 is privileged. The image runs as uid 65532; the Deployment grants only
`NET_BIND_SERVICE` so that user can bind it, because Kubernetes/containerd keeps
ports below 1024 privileged. A native development run needs root or the same
capability. There is deliberately no flag, environment variable or config key
to change the port.

The process binds `0.0.0.0:80`; the manifests enable `OTTERSCOPE_READ_AUTH`, so
the UI/API need a Bearer token and OTLP ingest needs a project ingest key. Auth
is off by default in the binary itself: before exposing a native run, pass
`-read-auth`. Read tokens cover the query surface; minting or revoking tokens
and share links, writing assertions, saved views or alerts, and on-demand
evaluation need an admin token. `otterscope token list` shows ids, scopes and
last use, never a secret. If the UI is reached by a hostname through a reverse
proxy, list that origin in `OTTERSCOPE_ALLOWED_ORIGINS` — the API checks
`Origin` and `Host` on every request and refuses a name it was not told about.
For reproducible deployments, put the rest in a config file —
`serve -config otterscope.json` (see
[docs/otterscope.example.json](docs/otterscope.example.json)) — or `OTTERSCOPE_*`
env vars; flags override env override the file.

To serve everything (UI, API, `/mcp`, share links, and the OTLP receiver) under
one path prefix — for instance behind an ingress that routes by path rather
than hostname — pass `-base-path /otterscope` (`OTTERSCOPE_BASE_PATH`, or
`"basePath"` in the config file; one setting whichever source you use). Point
exporters at `http://host/otterscope`; OTLP/HTTP appends `/v1/traces` itself,
so ingest lands at `/otterscope/v1/traces`. The UI bundle bakes its router and
API base at build time, so **a prefixed deployment requires an image built with
the matching build arg** — an image built without it targets the root path.
Build the UI with the same value (`OTTERSCOPE_BASE_PATH=/otterscope bun run
build` in `web/`) and pass the same value as the image's
`OTTERSCOPE_BASE_PATH` build arg. `deploy.sh` builds the root-path image, so a
prefixed deployment needs a manual build plus `k3d image import`; see the
optional Ingress section in [deploy/readme.md](deploy/readme.md).

If the image's baked base and the runtime `-base-path` disagree, the server
still starts — the API, OTLP receiver and MCP remain valid — but it logs a
prominent `WARN` and the UI will not work (its router and API calls target the
build-time path). The default is the root, and a request outside the prefix is
a 404 rather than the SPA, so a misrouted path cannot land on another
application's index.

Ingest is bounded. One OTLP request may carry at most 10,000 spans and touch
at most 1,000 distinct traces (`-ingest-max-spans`, `-ingest-max-trace-ids`);
a larger batch is refused whole with a 400 before anything is normalized. One
ingest key may send 50 batches a second by default (`-ingest-rate`,
`-ingest-burst`). A span id that already belongs to another service is
rejected and reported in OTLP partial success instead of overwriting that
service's step.

Want to poke around before wiring an agent? Seed realistic demo runs inside the
server pod: `kubectl -n otterscope exec deploy/otterscope -- otterscope sample`.

Want your agent to inspect *itself*? Otterscope ships a stateless [MCP server](docs/mcp.md) (protocol revision `2026-07-28`), so Claude Code and other MCP clients can query your runs, cost, and stats:

```sh
# substitute the host/port your deployment publishes
claude mcp add otterscope --transport http http://localhost:8080/mcp
```

## What you get

- **Runs, not span soup** — every trace becomes an agent run: steps, tool loops, per-run tokens and cost, error surfacing, live-tailing list with filters (status, model, service, project, time) and full-text search over message and tool content — all URL-shareable, and saveable as named views ("prod errors 24h") you can pin and reapply.
- **Run inspector** — step timeline with proportional duration bars; click any LLM call to read the actual messages in/out, token breakdowns (cache reads, reasoning), and cost; tool calls show arguments and results. Share any run as a public read-only link (opt-in per run, revocable) to hand a teammate a broken run without giving them your whole instance.

  ![Run detail — step timeline, assertion chips, and message inspector](docs/screenshots/run-detail.png)
- **Cost tracking** — maintained pricing table for major providers (override or extend with `serve -pricing yours.json`); unknown models show tokens, never fabricated costs.
- **Evals fused into the trace store** — assertions (`contains`, `regex`, `is_json`, latency/cost thresholds) and **LLM-as-judge** scored onto real runs at ingest or backfilled on demand. The judge endpoint, key, and model allow-list are server config (`-judge-url`, `OTTERSCOPE_JUDGE_KEY`, `-judge-models`), never per-assertion — so assertions can't name secrets to read or an arbitrary model to bill. On-demand backfills (`POST /api/assertions/evaluate`) must name a run, a time window, or a limit, and are capped per call (500 runs, 100 judge calls by default) with the skipped counts reported. No second product.
- **Compare view** — error rate, p50/p95 latency, cost, and assertion pass rates side-by-side across any two filters: this week vs last, model A vs model B, or **prompt version v2 vs v3** (captured from `gen_ai.prompt.name`/`version`). *"Did my prompt change make things worse?"* is one URL.
- **Alerting** — rules on error rate, cost, p95 latency, or an assertion's fail rate over a window; Otterscope POSTs a JSON notification to your webhook (Slack/Discord/…) when a rule starts firing and again when it resolves. A settle time requires the condition to hold before either notification, so a value sitting on the threshold can't page you twice a minute: alerts created through the API default to 0 (notify immediately), while the UI's draft starts at 5 minutes. Webhook URLs must be public HTTP(S) endpoints: loopback, link-local and private ranges are refused on creation and again on every POST, including through redirects, and a stored URL reads back redacted.

  ![Alerts — rules on error rate, cost, latency, or assertion failures firing to a webhook](docs/screenshots/alerts.png)

  ![Compare — two models side by side: error rate, latency percentiles, cost, assertion pass rates](docs/screenshots/compare.png)
- **Drop-in OTel compatibility** — normalizes the OTel GenAI conventions (both current dialects), OpenInference (OpenAI Agents SDK, CrewAI, LangChain), and the Vercel AI SDK. Raw payloads are retained, so old data benefits from future normalizer improvements.
- **Ask your agent about itself** — a built-in [MCP server](docs/mcp.md) (`POST /mcp`, stateless revision `2026-07-28`) lets Claude Code and other MCP clients query your runs, steps, cost, and stats; it needs the same read token when `-read-auth` is on. `claude mcp add otterscope --transport http http://localhost:8080/mcp` (the
port-forward above; `/healthz` and port 80 are inside the cluster).
- **Projects + ingest keys** for isolating multiple agents; **API tokens** scoped read or admin (`otterscope token add`, `-admin` for one that may mutate) to script against your data or build external dashboards — enforce them with `serve -read-auth` when you expose the instance, which also makes an ingest key mandatory. Optional retention sweep (`-retention 720h`) if you *want* to delete data; it removes the full-text index rows with the runs and vacuums, so deleted content stops being searchable.
- **Backups you can trust** — `otterscope backup -o snapshot.db` for a consistent copy (0600, like the database), and Otterscope auto-snapshots before applying a pending migration after the schema baseline, keeping the newest one. (A database from the old pre-baseline schema is refused, not migrated; see the upgrade note below.) The database, its `-wal`/`-shm` siblings and a directory Otterscope creates for them are owner-only too.
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

Otterscope is Apache-2.0, ships as one binary and one container image, and is built for individuals and small teams whose traces contain customer data they'd rather keep on their own disk. There is no paid tier, no hosted version, no metering and no seat fees — every feature is in the source you build from ([ADR-0005](docs/adr/0005-no-hosted-or-paid-offering.md)).

## Install

There is exactly one supported install: Kubernetes via k3d on macOS, using
[deploy/k8s/deploy.sh](deploy/k8s/deploy.sh) as described in the quick start and
[deploy/readme.md](deploy/readme.md). The image is built locally and never
published; there is no archive, package manager, registry step or Helm chart.

- **Kubernetes (k3d)** — Docker Desktop, `k3d` and `kubectl`; `deploy.sh`
  builds the image, imports it into the cluster and applies the manifests.
- **From source** — Bun + Rust, for working on the code rather than deploying
  it; see [Development](#development).

## Development

Rust backend, embedded Bun/React UI. The frontend must be built before the
binary if you want the real UI:

```sh
cd web && bun install && bun run build
cd ../rust && cargo build --release
sudo ./target/release/otterscope serve   # dev only; port 80 is privileged
```

There is no port setting: the one listener is always on 80, so a native run
needs root or `CAP_NET_BIND_SERVICE`. The Kubernetes Deployment runs as uid
65532 and adds only `NET_BIND_SERVICE`. Everything the server serves — UI, API,
`/mcp`, and OTLP ingest — is on that port.

`cargo build` also works without a frontend build: `build.rs` writes a
placeholder `web/dist/index.html`, so the API, OTLP receiver and `/mcp` still
run while the UI page says it was built without one. The binary lands at
`rust/target/release/otterscope` — run it as `./target/release/otterscope`
from `rust/`, or as `rust/target/release/otterscope` from the repository root.

The binary is a single deployable file, not a fully static one: SQLite is
compiled in (`rusqlite` with the `bundled` feature) and the UI is embedded
with `rust-embed`, so there is no database server, asset directory or sidecar
to deploy. SQLite comes from C, and a Linux build links glibc
2.39+ (the container image is the portable option), so "single binary" is the
promise — not "fully static".

Databases are plain SQLite. The schema is a single `STRICT` baseline
(`0001_initial.sql`) and future changes are new numbered migration files.
There is no compatible older schema to carry forward: a database created by
an older (Go-era or pre-baseline) Otterscope is refused rather than
half-migrated, so start a new database and re-ingest. Back up before switching
from any older build. See
[docs/adr/0008-sqlite-schema-redesign.md](docs/adr/0008-sqlite-schema-redesign.md).

Tests: `cd rust && cargo test` and `cd web && bun test`. Quality gates:
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings` and `cargo test` in `rust/`; `bun run lint`, `bun test` and `bun run
build` in `web/`. The HTTP contract is [docs/api-rust.md](docs/api-rust.md) and
MCP is [docs/mcp.md](docs/mcp.md). See
[docs/development.md](docs/development.md) for the architecture map and
[docs/workflow.md](docs/workflow.md) for the contribution process.

Contributions are welcome and there's nothing to sign — no CLA, no copyright
assignment. GitHub Issues are disabled on this repository; open a PR with a
focused change.

## License

[Apache-2.0](license)
