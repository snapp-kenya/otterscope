# Otterscope

**Lightweight, self-hosted observability and evals for AI agents.**

One self-contained binary. One SQLite file. No ClickHouse, no Redis, no S3, no seat fees, no trace metering. Point your agent's OpenTelemetry exporter at Otterscope and get an agent-run-first view of everything it did — LLM calls, tool calls, loops, errors, latency, and cost — plus assertions and LLM-as-judge evals scored onto your real production runs. All on your own disk, with unlimited retention.

> Think *Plausible Analytics, but for AI agents*.

![Runs list — live-tailing agent runs with status, duration, models, tokens, and cost](docs/screenshots/runs-list.png)

## Quick start

```sh
# grab a release archive (or build from source; see Development), then:
./otterscope serve
```

The server has **one HTTP listener, always on port 80**. The UI, JSON API,
`/mcp`, `/healthz`, share links and OTLP ingest (`POST /v1/traces`) all live
there. TLS and how the port is exposed are the platform's job: behind Istio the
service is reached on port 80 and Istio terminates TLS.

Port 80 is privileged. A native run therefore needs privilege — run it as root
or grant the binary `CAP_NET_BIND_SERVICE` on Linux (`sudo setcap
'cap_net_bind_service=+ep' ./otterscope`). Containers are different: Docker's
runtime sets `net.ipv4.ip_unprivileged_port_start=0`, so the non-root image can
bind 80 there without the capability, while Kubernetes/containerd keeps the
default boundary and needs `CAP_NET_BIND_SERVICE`. There is deliberately no
flag, environment variable or config key to change the port.

The process binds `0.0.0.0:80`, so add `-read-auth` before anything else can
reach it; without it the API is a full read/write surface for anyone who can
reach the port. For reproducible deployments, put the rest in a config file —
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
the matching build arg** — the stock image is built for the root path. Build
the UI with the same value (`OTTERSCOPE_BASE_PATH=/otterscope bun run build` in
`web/`) and the image with:

```sh
docker build --build-arg OTTERSCOPE_BASE_PATH=/otterscope -t <img> .
```

If the image's baked base and the runtime `-base-path` disagree, the server
still starts — the API, OTLP receiver and MCP remain valid — but it logs a
prominent `WARN` and the UI will not work (its router and API calls target the
build-time path). The default is the root, and a request outside the prefix is
a 404 rather than the SPA, so a misrouted path cannot land on another
application's index.

Auth is off by default. Before you expose the instance, pass `-read-auth`: the
UI and API then need a Bearer token. Mint the first one with `otterscope token
add local` — it is shown only at creation and stored only as a SHA-256 digest;
`-admin` mints a token that may also mutate. Read tokens cover the query
surface; minting or revoking tokens and share links, writing assertions, saved
views or alerts, and on-demand evaluation need an admin token. `otterscope
token list` shows ids, scopes and last use, never a secret. With read auth on,
OTLP ingest also requires a project ingest key (`otterscope project add
my-agent`); the keyless default project is closed. If the UI is reached by a
hostname through a reverse proxy, list that origin in
`OTTERSCOPE_ALLOWED_ORIGINS` — the API checks `Origin` and `Host` on every
request and refuses a name it was not told about.

Docker works the same way. The image is a distroless build running as uid
65532. Docker allows that user to bind 80 because the runtime sets
`net.ipv4.ip_unprivileged_port_start=0`; the command below still adds only
`CAP_NET_BIND_SERVICE` and drops everything else, so the same invocation also
works on runtimes that keep privileged ports privileged (Kubernetes/containerd).
The host-side port is yours to choose — this example publishes loopback port
8080 to the container's 80:

```sh
docker run --read-only --cap-drop ALL --cap-add NET_BIND_SERVICE \
  -p 127.0.0.1:8080:80 -v otterscope:/data \
  ghcr.io/otterscope/otterscope serve -db /data/otterscope.db
```

[docker-compose.yml](docker-compose.yml) is the maintained version of that
command, with read auth on and the UI published to 127.0.0.1 only. A named
volume inherits `/data`'s ownership from the image; a host bind mount has to
be owned by 65532.

Point any OTel-instrumented agent at the base URL of your deployment:

```sh
# exporters append /v1/traces themselves; use the host/port you published
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:8080
```

With `-read-auth` on, also send a project ingest key (mint one with
`otterscope project add my-agent`); keyless ingest is only accepted when read
auth is off:

```sh
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer <ingest key>"
```

Open **http://localhost:8080** (for a host binary listening on 80 it is
**http://localhost**). No account by default, no config file, no other
services.

Ingest is bounded. One OTLP request may carry at most 10,000 spans and touch
at most 1,000 distinct traces (`-ingest-max-spans`, `-ingest-max-trace-ids`);
a larger batch is refused whole with a 400 before anything is normalized. One
ingest key may send 50 batches a second by default (`-ingest-rate`,
`-ingest-burst`). A span id that already belongs to another service is
rejected and reported in OTLP partial success instead of overwriting that
service's step.

Want to poke around before wiring an agent? `./otterscope sample` seeds realistic demo runs.

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
- **Ask your agent about itself** — a built-in [MCP server](docs/mcp.md) (`POST /mcp`, stateless revision `2026-07-28`) lets Claude Code and other MCP clients query your runs, steps, cost, and stats; it needs the same read token when `-read-auth` is on. `claude mcp add otterscope --transport http http://localhost/mcp` (substitute the port your deployment publishes).
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

Otterscope is Apache-2.0, ships as one binary and one container image, and is built for individuals and small teams whose traces contain customer data they'd rather keep on their own disk. There is no paid tier, no hosted version, no metering and no seat fees — every feature is in the binary you just downloaded ([ADR-0005](docs/adr/0005-no-hosted-or-paid-offering.md)).

## Install

- **Binaries** — [Releases](https://github.com/otterscope/otterscope/releases) (Linux amd64/arm64, macOS arm64, Windows amd64). The Linux binaries link
  glibc 2.39+, so Docker is the portable option.
- **Docker** — `ghcr.io/otterscope/otterscope`
- **Kubernetes** — plain YAML plus `kustomize`, no Helm; see [deploy/readme.md](deploy/readme.md)
- **From source** — Bun + Rust; see [Development](#development)

## Development

Rust backend, embedded Bun/React UI. The frontend must be built before the
binary if you want the real UI:

```sh
cd web && bun install && bun run build
cd ../rust && cargo build --release
sudo ./target/release/otterscope serve   # port 80 is privileged
```

There is no port setting: the one listener is always on 80, so a native run
needs root or `CAP_NET_BIND_SERVICE`. In a container Docker permits uid 65532 to
bind 80 without the capability; Kubernetes/containerd needs it. Everything the
server serves — UI, API, `/mcp`, and OTLP ingest — is on that port.

`cargo build` also works without a frontend build: `build.rs` writes a
placeholder `web/dist/index.html`, so the API, OTLP receiver and `/mcp` still
run while the UI page says it was built without one. The binary lands at
`rust/target/release/otterscope` — run it as `./target/release/otterscope`
from `rust/`, or as `rust/target/release/otterscope` from the repository root.

The binary is a single deployable file, not a fully static one: SQLite is
compiled in (`rusqlite` with the `bundled` feature) and the UI is embedded
with `rust-embed`, so there is no database server, asset directory or sidecar
to deploy. SQLite comes from C, and the Linux release binaries link glibc
2.39+ (Docker is the portable option), so "single binary" is the promise —
not "fully static".

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
